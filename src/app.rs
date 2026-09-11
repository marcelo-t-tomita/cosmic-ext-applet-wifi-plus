//! The panel itself: an Omarchy-style Wi-Fi popup rendered as a COSMIC applet.

use std::sync::LazyLock;
use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::applet::{menu_button, padded_control};
use cosmic::iced::core::window;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::destroy_popup;
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::widget::{
    Id, autosize, button, column, container, divider, icon::from_name, indeterminate_circular,
    qr_code, row, scrollable, secure_input, space, text, text_input, toggler,
};
use cosmic::Element;

use crate::config::Settings;
use crate::fl;
use crate::model::{self, PingSamples, Throughput};
use crate::net::{self, Failure, Kind, Security};

pub const APP_ID: &str = "io.github.marcelo_t_tomita.CosmicWifiPlus";

/// Polls while the popup is open. The bar icon needs far less, so the closed
/// panel settles into a slower beat that never forces a scan.
const TICK: Duration = Duration::from_secs(2);
const CLOSED_TICKS_PER_POLL: u32 = 4;
const RESCAN_TICKS: u32 = 5;
const PHRASE_TICKS: u32 = 5;
const POPUP_WIDTH: f32 = 420.0;
const MAX_LABEL_CHARS: usize = 18;

/// The bar widget changes width when the SSID label is toggled, so the applet
/// surface has to renegotiate its size with the panel rather than stay at the
/// icon's fixed square -- without this the label is simply clipped away.
static AUTOSIZE_MAIN_ID: LazyLock<Id> = LazyLock::new(|| Id::new("autosize-main"));

pub fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<WifiPanel>(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Connect,
    Disconnect,
    Forget,
}

/// One round of every command the panel reads. Gathered off the UI thread and
/// delivered as a single message so the popup never renders a half-updated mix
/// of old and new readings.
#[derive(Debug, Clone, Default)]
pub struct Poll {
    status: net::Status,
    rows: Vec<net::WifiRow>,
    band: net::BandStatus,
    dns: String,
    wifi_enabled: bool,
    router_ping: Option<f64>,
    internet_ping: Option<f64>,
    full: bool,
}

#[derive(Debug, Clone)]
pub enum SpeedTest {
    Idle,
    Running { upload: bool },
    Done { mbps: f64, upload: bool },
    Failed(Failure),
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(window::Id),
    Tick,
    Polled(Box<Poll>),
    ToggleWifi(bool),
    RowPressed(String),
    CollapsePrompt,
    PasswordChanged(String),
    IdentityChanged(String),
    TogglePasswordVisible,
    SubmitPassword,
    Forget(String),
    ActionDone(Result<(), Failure>),
    ToggleBandAuto(bool),
    SetBand(String),
    SetDns(String),
    ShowQr,
    QrReady(Option<String>),
    HideQr,
    RunSpeedTest(bool),
    SpeedTestDone(Result<f64, Failure>),
    OpenCaptivePortal,
    ToggleSsidLabel(bool),
}

pub struct WifiPanel {
    core: Core,
    popup: Option<window::Id>,

    status: net::Status,
    rows: Vec<net::WifiRow>,
    band: net::BandStatus,
    dns: String,
    wifi_enabled: bool,

    throughput: Throughput,
    pings: PingSamples,

    tick: u32,
    phrase_index: usize,
    scanning: bool,

    /// The SSID whose passphrase prompt is expanded, if any.
    prompt: Option<String>,
    password: String,
    identity: String,
    password_hidden: bool,

    busy: Option<(String, ActionKind)>,
    /// The SSID a failure belongs to, and the phrase to show on its row.
    failure: Option<(String, Failure)>,

    qr: Option<qr_code::Data>,
    speedtest: SpeedTest,
    settings: Settings,

    icon_name: String,
}

impl Default for WifiPanel {
    fn default() -> Self {
        Self {
            core: Core::default(),
            popup: None,
            status: net::Status::default(),
            rows: Vec::new(),
            band: net::BandStatus::default(),
            dns: "DHCP".to_string(),
            wifi_enabled: true,
            throughput: Throughput::default(),
            pings: PingSamples::default(),
            tick: 0,
            phrase_index: 0,
            scanning: false,
            prompt: None,
            password: String::new(),
            identity: String::new(),
            password_hidden: true,
            busy: None,
            failure: None,
            qr: None,
            speedtest: SpeedTest::Idle,
            settings: Settings::default(),
            icon_name: "network-wireless-offline-symbolic".to_string(),
        }
    }
}

/// Every reading the panel needs, taken in one pass on a blocking thread.
fn poll_blocking(full: bool, rescan: bool) -> Poll {
    let status = net::status();
    let (router_ping, internet_ping) = net::pings(&status.gateway);

    if !full {
        return Poll {
            status,
            router_ping,
            internet_ping,
            wifi_enabled: net::wifi_enabled(),
            ..Default::default()
        };
    }

    let band = net::band_status(&status);
    let dns = net::dns_provider(&band.profile);

    Poll {
        rows: net::scan(rescan),
        band,
        dns,
        wifi_enabled: net::wifi_enabled(),
        status,
        router_ping,
        internet_ping,
        full: true,
    }
}

fn poll_task(full: bool, rescan: bool) -> Task<Message> {
    cosmic::task::future(async move {
        let poll = tokio::task::spawn_blocking(move || poll_blocking(full, rescan))
            .await
            .unwrap_or_default();
        Message::Polled(Box::new(poll))
    })
}

/// Runs one nmcli-backed action off the UI thread.
fn action_task<F>(work: F) -> Task<Message>
where
    F: FnOnce() -> Result<(), Failure> + Send + 'static,
{
    cosmic::task::future(async move {
        let result = tokio::task::spawn_blocking(work)
            .await
            .unwrap_or(Err(Failure::ConnectionFailed));
        Message::ActionDone(result)
    })
}

/// Turns a failure into the phrase shown on the offending row.
fn failure_text(failure: Failure) -> String {
    match failure {
        Failure::PassphraseRequired => fl!("passphrase-required"),
        Failure::WrongPassword => fl!("wrong-password"),
        Failure::NetworkLost => fl!("network-lost"),
        Failure::ConnectionFailed => fl!("connection-failed"),
        Failure::DisconnectFailed => fl!("disconnect-failed"),
        Failure::ForgetFailed => fl!("forget-failed"),
        Failure::WifiToggleFailed => fl!("wifi-toggle-failed"),
        Failure::NoProfile => fl!("no-profile"),
        Failure::BandSetFailed => fl!("band-set-failed"),
        Failure::BandReverted => fl!("band-reverted"),
        Failure::DnsSetFailed => fl!("dns-set-failed"),
        Failure::SpeedTestFailed => fl!("speed-test-failed"),
        Failure::CurlRequired => fl!("curl-required"),
    }
}

fn latency_text(ms: f64, has_samples: bool) -> String {
    match model::latency(ms, has_samples) {
        model::Latency::Pending => fl!("no-data"),
        model::Latency::TimedOut => fl!("timeout"),
        model::Latency::Milliseconds(ms) => {
            fl!("milliseconds", value = model::format_latency_value(ms))
        }
    }
}

fn packet_loss_text(percent: i32, has_samples: bool) -> String {
    if !has_samples {
        return fl!("no-data");
    }
    format!("{}%", percent.max(0))
}

impl WifiPanel {
    fn restricted(&self) -> bool {
        // Associated but with no route to the probe: the same "limited access"
        // state Omarchy surfaces, inferred here from a failing internet ping on
        // an otherwise live link.
        self.status.connected()
            && self.pings.has_internet_samples()
            && self.pings.internet_packet_loss() == 100
    }

    fn update_icon(&mut self) {
        self.icon_name =
            model::connection_icon(self.status.kind, self.status.signal, self.restricted())
                .to_string();
    }

    /// The SSID to show beside the bar icon, or `None` to render the icon
    /// alone. A vertical panel is only as wide as its icons, so the label is
    /// dropped there rather than squeezed; Ethernet and a dead link have no
    /// name worth the space either.
    fn bar_label(&self) -> Option<String> {
        if !self.settings.show_ssid || !self.core.applet.is_horizontal() {
            return None;
        }
        if self.status.kind != Kind::Wifi || self.status.ssid.is_empty() {
            return None;
        }

        let ssid = &self.status.ssid;
        if ssid.chars().count() <= MAX_LABEL_CHARS {
            return Some(ssid.clone());
        }

        // Count in chars, not bytes: an SSID is arbitrary UTF-8.
        let clipped: String = ssid.chars().take(MAX_LABEL_CHARS - 1).collect();
        Some(format!("{clipped}\u{2026}"))
    }

    fn settings_section(&self) -> Element<'_, Message> {
        padded_control(
            row::with_children(vec![
                text::body(fl!("show-network-name"))
                    .width(Length::Fill)
                    .into(),
                toggler(self.settings.show_ssid)
                    .on_toggle(Message::ToggleSsidLabel)
                    .into(),
            ])
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .into()
    }

    fn network(&self, ssid: &str) -> Option<&net::WifiRow> {
        self.rows.iter().find(|row| row.ssid == ssid)
    }

    fn connection_phrase(&self) -> String {
        let key = model::CONNECTION_PHRASES[self.phrase_index % model::CONNECTION_PHRASES.len()];

        // Fluent ids must be literals for the macro, so the rotation maps its
        // key onto one of the seven calls rather than looking one up.
        match key {
            "phrase-wiring-bits" => fl!("phrase-wiring-bits"),
            "phrase-handling-packets" => fl!("phrase-handling-packets"),
            "phrase-sorting-frames" => fl!("phrase-sorting-frames"),
            "phrase-hauling-bytes" => fl!("phrase-hauling-bytes"),
            "phrase-routing-crumbs" => fl!("phrase-routing-crumbs"),
            "phrase-counting-collisions" => fl!("phrase-counting-collisions"),
            _ => fl!("phrase-bending-light"),
        }
    }

    fn band_title(&self) -> String {
        match model::band_header_value(&self.band.selected, &self.band.current) {
            Some(band) => fl!("wifi-band-current", band = band),
            None => fl!("wifi-band"),
        }
    }

    fn header_detail(&self) -> String {
        // Wi-Fi band state belongs in the selector section, not beside the hero
        // name. Ethernet has no equivalent selector, so keep its negotiated
        // link speed here.
        if self.status.kind == Kind::Ethernet {
            model::format_header_speed(&self.status.eth_speed)
        } else {
            String::new()
        }
    }

    fn hero(&self) -> Element<'_, Message> {
        let title = match self.status.kind {
            Kind::Wifi => {
                if self.status.ssid.is_empty() {
                    fl!("wifi")
                } else {
                    self.status.ssid.clone()
                }
            }
            Kind::Ethernet => fl!("ethernet"),
            Kind::Disconnected => fl!("disconnected"),
        };

        let detail = self.header_detail();
        let title = if detail.is_empty() {
            title
        } else {
            format!("{title} ({detail})")
        };

        let meta = if self.restricted() {
            fl!("limited-internet-access")
        } else if self.status.connected() {
            self.connection_phrase().to_uppercase()
        } else {
            fl!("not-connected")
        };

        let labels = column::with_children(vec![
            text::title4(title).into(),
            text::caption(meta).into(),
        ])
        .width(Length::Fill)
        .spacing(2);

        let mut actions: Vec<Element<'_, Message>> = Vec::new();

        if self.status.kind == Kind::Wifi && self.status.connected() {
            actions.push(
                button::icon(from_name("send-to-symbolic").size(16))
                    .icon_size(16)
                    .on_press(Message::ShowQr)
                    .into(),
            );
            actions.push(
                button::icon(from_name("speedometer-symbolic").size(16))
                    .icon_size(16)
                    .on_press(Message::RunSpeedTest(false))
                    .into(),
            );
        }

        actions.push(
            toggler(self.wifi_enabled)
                .on_toggle(Message::ToggleWifi)
                .into(),
        );

        row::with_children(vec![
            from_name(&*self.icon_name).size(32).symbolic(true).into(),
            labels.into(),
            row::with_children(actions)
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
        ])
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    }

    fn metric<'a>(&self, label: String, value: String) -> Element<'a, Message> {
        row::with_children(vec![
            text::caption(label).into(),
            text::body(value)
                .width(Length::Fill)
                .align_x(Alignment::End)
                .into(),
        ])
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    fn metric_row<'a>(
        &self,
        left: Element<'a, Message>,
        right: Element<'a, Message>,
    ) -> Element<'a, Message> {
        row::with_children(vec![
            container(left).width(Length::Fill).into(),
            container(right).width(Length::Fill).into(),
        ])
        .spacing(20)
        .into()
    }

    /// Connection details: transfer metrics first, then IP/Gateway. The rows
    /// stay mounted and read "--" until a sample arrives so the popup doesn't
    /// reflow a beat after it opens.
    fn metrics(&self) -> Element<'_, Message> {
        let has_samples = self.pings.has_internet_samples();
        let has_transfer = self.status.rx_bytes.is_some();

        let rate = |value: f64| {
            if has_transfer {
                model::format_rate(value)
            } else {
                fl!("no-data")
            }
        };
        let total = |value: Option<u64>| {
            value
                .map(|v| model::format_bytes(v as f64))
                .unwrap_or_else(|| fl!("no-data"))
        };
        let or_dash = |value: &str| {
            if value.is_empty() {
                fl!("no-data")
            } else {
                value.to_string()
            }
        };

        column::with_children(vec![
            self.metric_row(
                self.metric(
                    fl!("ping"),
                    latency_text(self.pings.internet_latency(), has_samples),
                ),
                self.metric(
                    fl!("packet-loss"),
                    packet_loss_text(self.pings.internet_packet_loss(), has_samples),
                ),
            ),
            self.metric_row(
                self.metric(fl!("receiving"), rate(self.throughput.download_rate)),
                self.metric(fl!("sending"), rate(self.throughput.upload_rate)),
            ),
            self.metric_row(
                self.metric(fl!("downloaded"), total(self.status.rx_bytes)),
                self.metric(fl!("uploaded"), total(self.status.tx_bytes)),
            ),
            self.metric_row(
                self.metric(fl!("ip-address"), or_dash(&self.status.ip)),
                self.metric(fl!("gateway"), or_dash(&self.status.gateway)),
            ),
        ])
        .spacing(6)
        .into()
    }

    /// Only on Wi-Fi, and only when the network answers on more than one band
    /// -- a single-band AP has nothing to toggle.
    fn band_section(&self) -> Option<Element<'_, Message>> {
        if self.status.kind != Kind::Wifi || self.band.available.len() < 2 {
            return None;
        }

        let pinned = self.band.selected != "auto";

        let header = row::with_children(vec![
            text::caption(self.band_title()).width(Length::Fill).into(),
            text::caption(fl!("automatic")).into(),
            toggler(!pinned).on_toggle(Message::ToggleBandAuto).into(),
        ])
        .spacing(8)
        .align_y(Alignment::Center);

        let mut section = column::with_capacity(2).spacing(10).push(header);

        if pinned {
            let pills: Vec<Element<'_, Message>> = self
                .band
                .available
                .iter()
                .map(|band| {
                    let label = model::band_label(band);
                    let selected = &self.band.selected == band;
                    let pill = if selected {
                        button::suggested(label)
                    } else {
                        button::standard(label)
                    };
                    pill.width(Length::Fill)
                        .on_press(Message::SetBand(band.clone()))
                        .into()
                })
                .collect();

            section = section.push(row::with_children(pills).spacing(6));
        }

        Some(section.into())
    }

    fn dns_section(&self) -> Element<'_, Message> {
        // "Custom" is a state the panel can report but not set: it means DNS
        // servers were configured outside the panel, and clicking it would have
        // nothing to apply.
        let pills: Vec<Element<'_, Message>> = [
            ("DHCP", fl!("dns-dhcp")),
            ("Cloudflare", fl!("dns-cloudflare")),
            ("Google", fl!("dns-google")),
            ("Custom", fl!("dns-custom")),
        ]
        .into_iter()
        .map(|(provider, label)| {
                let selected = self.dns == provider;
                let pill = if selected {
                    button::suggested(label)
                } else {
                    button::standard(label)
                };

                let pill = pill.width(Length::Fill);

                if provider == "Custom" {
                    pill.into()
                } else {
                    pill.on_press(Message::SetDns(provider.to_string())).into()
                }
            })
            .collect();

        column::with_children(vec![
            text::caption(fl!("dns-provider")).into(),
            row::with_children(pills).spacing(6).into(),
        ])
        .spacing(10)
        .into()
    }

    fn status_text(&self, row: &net::WifiRow) -> Option<(String, bool)> {
        if let Some((ssid, kind)) = &self.busy {
            if ssid == &row.ssid {
                return Some((
                    match kind {
                        ActionKind::Connect => fl!("connecting"),
                        ActionKind::Disconnect => fl!("disconnecting"),
                        ActionKind::Forget => fl!("forgetting"),
                    },
                    false,
                ));
            }
        }

        if row.connected {
            if self.restricted() {
                return Some((fl!("sign-in-required"), true));
            }
            return Some((fl!("connected"), false));
        }

        if let Some((ssid, reason)) = &self.failure {
            if ssid == &row.ssid {
                return Some((failure_text(*reason), true));
            }
        }

        None
    }

    /// The widgets inside one list entry.
    fn row_content(&self, row: &net::WifiRow) -> Vec<Element<'_, Message>> {
        let mut content: Vec<Element<'_, Message>> = vec![
            from_name(model::wifi_icon_for(row.signal))
                .size(20)
                .symbolic(true)
                .into(),
        ];

        if row.security.requires_credentials() {
            content.push(from_name("system-lock-screen-symbolic").size(14).symbolic(true).into());
        }

        content.push(
            text::body(if row.ssid.is_empty() {
                fl!("hidden-network")
            } else {
                row.ssid.clone()
            })
            .width(Length::Fill)
            .into(),
        );

        if let Some((status, _urgent)) = self.status_text(row) {
            content.push(text::caption(status).into());
        }

        if row.known && !row.connected {
            content.push(
                button::icon(from_name("edit-delete-symbolic").size(14))
                    .icon_size(14)
                    .on_press(Message::Forget(row.ssid.clone()))
                    .into(),
            );
        }

        content
    }

    fn password_prompt(&self, row: &net::WifiRow) -> Element<'_, Message> {
        let mut fields = column::with_capacity(3).spacing(8);

        if row.security == Security::Enterprise {
            fields = fields.push(
                text_input(fl!("identity-placeholder"), &self.identity)
                    .on_input(Message::IdentityChanged)
                    .on_submit(|_| Message::SubmitPassword),
            );
        }

        fields = fields.push(
            secure_input(
                fl!("passphrase-placeholder"),
                &self.password,
                Some(Message::TogglePasswordVisible),
                self.password_hidden,
            )
            .on_input(Message::PasswordChanged)
            .on_submit(|_| Message::SubmitPassword),
        );

        fields = fields.push(
            row::with_children(vec![
                button::standard(fl!("cancel"))
                    .on_press(Message::CollapsePrompt)
                    .into(),
                button::suggested(fl!("connect"))
                    .on_press(Message::SubmitPassword)
                    .into(),
            ])
            .spacing(8),
        );

        padded_control(fields).into()
    }

    fn network_list(&self) -> Element<'_, Message> {
        if !self.wifi_enabled {
            return padded_control(text::caption(fl!("wifi-is-off"))).into();
        }

        if self.rows.is_empty() {
            return padded_control(text::caption(fl!("scanning-wifi"))).into();
        }

        let mut list = column::with_capacity(self.rows.len()).spacing(2);

        for (index, row) in self.rows.iter().enumerate() {
            if let Some(section) = model::wifi_section(&self.rows, index) {
                let title = match section {
                    model::Section::Known => fl!("known-networks"),
                    model::Section::Other => fl!("other-networks"),
                };
                list = list.push(padded_control(text::caption(title)));
            }

            let entry = menu_button(
                row::with_children(self.row_content(row))
                    .spacing(8)
                    .align_y(Alignment::Center),
            )
            .on_press(Message::RowPressed(row.ssid.clone()));

            list = list.push(entry);

            if self.prompt.as_deref() == Some(row.ssid.as_str()) {
                list = list.push(self.password_prompt(row));
            }
        }

        scrollable(list).height(Length::Fixed(260.0)).into()
    }

    fn speedtest_row(&self) -> Option<Element<'_, Message>> {
        let content = match &self.speedtest {
            SpeedTest::Idle => return None,
            SpeedTest::Running { upload } => row::with_children(vec![
                text::caption(if *upload { fl!("upload-test") } else { fl!("download-test") })
                    .width(Length::Fill)
                    .into(),
                indeterminate_circular().size(16.0).into(),
            ]),
            SpeedTest::Done { mbps, upload } => row::with_children(vec![
                text::caption(if *upload { fl!("upload") } else { fl!("download") })
                    .width(Length::Fill)
                    .into(),
                text::body(fl!("megabits-per-second", value = format!("{mbps:.0}"))).into(),
                button::standard(if *upload { fl!("test-download") } else { fl!("test-upload") })
                    .on_press(Message::RunSpeedTest(!*upload))
                    .into(),
            ]),
            SpeedTest::Failed(reason) => row::with_children(vec![
                text::caption(failure_text(*reason)).width(Length::Fill).into(),
                button::standard(fl!("retry"))
                    .on_press(Message::RunSpeedTest(false))
                    .into(),
            ]),
        };

        Some(content.spacing(8).align_y(Alignment::Center).into())
    }

    fn qr_panel<'a>(&self, data: &'a qr_code::Data) -> Element<'a, Message> {
        column::with_children(vec![
            text::caption(fl!("scan-to-join")).into(),
            container(qr_code(data).cell_size(6))
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .into(),
            button::standard(fl!("close"))
                .on_press(Message::HideQr)
                .into(),
        ])
        .spacing(10)
        .align_x(Alignment::Center)
        .into()
    }
}

impl cosmic::Application for WifiPanel {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        (
            Self {
                core,
                settings: Settings::load(),
                ..Default::default()
            },
            poll_task(false, false),
        )
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        cosmic::iced::time::every(TICK).map(|_| Message::Tick)
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    self.prompt = None;
                    self.qr = None;
                    return destroy_popup(id);
                }

                return Task::batch(vec![
                    poll_task(true, true),
                    cosmic::surface::surface_task(cosmic::surface::action::app_popup(
                        |_: &Self| Default::default(),
                        |app: &mut Self| {
                            let new_id = window::Id::unique();
                            app.popup = Some(new_id);
                            app.scanning = true;

                            app.core.applet.get_popup_settings(
                                app.core.main_window_id().unwrap(),
                                new_id,
                                None,
                                None,
                                None,
                            )
                        },
                        None,
                    )),
                ]);
            }

            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                    self.prompt = None;
                    self.qr = None;
                }
            }

            Message::Tick => {
                self.tick = self.tick.wrapping_add(1);

                if self.tick % PHRASE_TICKS == 0 {
                    self.phrase_index = self.phrase_index.wrapping_add(1);
                }

                let open = self.popup.is_some();
                if open {
                    // Keep the scanner warm while the panel is on screen, but
                    // not on every tick -- a forced scan stalls nmcli for a
                    // beat and would make the list flicker.
                    return poll_task(true, self.tick % RESCAN_TICKS == 0);
                }
                if self.tick % CLOSED_TICKS_PER_POLL == 0 {
                    return poll_task(false, false);
                }
            }

            Message::Polled(poll) => {
                let poll = *poll;
                let iface = poll.status.iface.clone();

                if let (Some(rx), Some(tx)) = (poll.status.rx_bytes, poll.status.tx_bytes) {
                    self.throughput.sample(&iface, rx, tx);
                } else {
                    self.throughput.reset();
                }

                self.pings
                    .sample(&iface, poll.router_ping, poll.internet_ping);

                self.wifi_enabled = poll.wifi_enabled;
                self.status = poll.status;

                if poll.full {
                    self.rows = poll.rows;
                    self.band = poll.band;
                    self.dns = poll.dns;
                    self.scanning = false;
                }

                self.update_icon();
            }

            Message::ToggleWifi(enabled) => {
                self.wifi_enabled = enabled;
                return action_task(move || {
                    if net::set_wifi_enabled(enabled) {
                        Ok(())
                    } else {
                        Err(Failure::WifiToggleFailed)
                    }
                });
            }

            Message::RowPressed(ssid) => {
                self.failure = None;

                let Some(row) = self.network(&ssid).cloned() else {
                    return Task::none();
                };

                if row.connected {
                    self.busy = Some((ssid.clone(), ActionKind::Disconnect));
                    return action_task(move || net::disconnect(&ssid));
                }

                // A saved profile already carries its secret, so it connects
                // straight away; only an unknown credentialed network opens the
                // prompt.
                if row.known || !row.security.requires_credentials() {
                    self.busy = Some((ssid.clone(), ActionKind::Connect));
                    let known = row.known;
                    let target = ssid.clone();
                    return action_task(move || {
                        if known {
                            net::connect_known(&target)
                        } else {
                            net::connect_open(&target)
                        }
                    });
                }

                self.prompt = Some(ssid);
                self.password.clear();
                self.identity.clear();
                self.password_hidden = true;
            }

            Message::CollapsePrompt => {
                self.prompt = None;
                self.password.clear();
                self.identity.clear();
                self.failure = None;
            }

            Message::PasswordChanged(value) => self.password = value,
            Message::IdentityChanged(value) => self.identity = value,
            Message::TogglePasswordVisible => self.password_hidden = !self.password_hidden,

            Message::SubmitPassword => {
                let Some(ssid) = self.prompt.clone() else {
                    return Task::none();
                };
                let Some(row) = self.network(&ssid).cloned() else {
                    return Task::none();
                };

                let password = std::mem::take(&mut self.password);
                let identity = self.identity.clone();
                self.busy = Some((ssid.clone(), ActionKind::Connect));
                self.failure = None;
                self.password_hidden = true;

                return action_task(move || {
                    if row.security == Security::Enterprise {
                        net::connect_enterprise(&ssid, &identity, &password)
                    } else {
                        net::connect_psk(&ssid, &password)
                    }
                });
            }

            Message::Forget(ssid) => {
                self.busy = Some((ssid.clone(), ActionKind::Forget));
                return action_task(move || net::forget(&ssid));
            }

            Message::ActionDone(result) => {
                let was = self.busy.take();
                let ssid = was.as_ref().map(|(ssid, _)| ssid.clone()).unwrap_or_default();

                match result {
                    Ok(()) => {
                        self.failure = None;
                        // A successful connect closes the prompt; the row
                        // repaints as "Connected" on the next poll.
                        if matches!(was, Some((_, ActionKind::Connect))) {
                            self.prompt = None;
                        }
                    }
                    Err(reason) => {
                        // A wrong passphrase leaves the prompt open so it can
                        // be corrected -- nmcli has already saved the bad
                        // profile, and reconnecting overwrites the stored PSK.
                        if reason.needs_passphrase() && !ssid.is_empty() {
                            self.prompt = Some(ssid.clone());
                        }
                        self.failure = Some((ssid, reason));
                    }
                }

                return poll_task(self.popup.is_some(), false);
            }

            Message::ToggleBandAuto(auto) => {
                let profile = self.band.profile.clone();
                let target = if auto {
                    "auto".to_string()
                } else {
                    // Pinning from Automatic keeps the radio where it already
                    // is rather than moving it.
                    self.band.current.clone()
                };

                if target.is_empty() {
                    return Task::none();
                }

                return action_task(move || net::set_band(&profile, &target));
            }

            Message::SetBand(band) => {
                let profile = self.band.profile.clone();
                return action_task(move || net::set_band(&profile, &band));
            }

            Message::SetDns(provider) => {
                let profile = self.band.profile.clone();
                let servers = match provider.as_str() {
                    "Cloudflare" => net::DNS_CLOUDFLARE,
                    "Google" => net::DNS_GOOGLE,
                    _ => "",
                }
                .to_string();

                self.dns = provider;
                return action_task(move || net::set_dns(&profile, &servers));
            }

            Message::ShowQr => {
                let iface = self.status.iface.clone();
                return cosmic::task::future(async move {
                    let payload = tokio::task::spawn_blocking(move || net::wifi_qr_payload(&iface))
                        .await
                        .unwrap_or(None);
                    Message::QrReady(payload)
                });
            }

            Message::QrReady(payload) => {
                self.qr = payload.and_then(|p| qr_code::Data::new(p).ok());
            }

            Message::HideQr => self.qr = None,

            Message::RunSpeedTest(upload) => {
                self.speedtest = SpeedTest::Running { upload };
                let iface = self.status.iface.clone();

                return cosmic::task::future(async move {
                    let result =
                        tokio::task::spawn_blocking(move || net::speedtest(&iface, upload))
                            .await
                            .unwrap_or(Err(Failure::SpeedTestFailed));
                    Message::SpeedTestDone(result)
                });
            }

            Message::SpeedTestDone(result) => {
                let upload = matches!(self.speedtest, SpeedTest::Running { upload: true });
                self.speedtest = match result {
                    Ok(mbps) => SpeedTest::Done { mbps, upload },
                    Err(reason) => SpeedTest::Failed(reason),
                };
            }

            Message::ToggleSsidLabel(show) => {
                self.settings.set_show_ssid(show);
            }

            Message::OpenCaptivePortal => {
                // A known plain-HTTP endpoint lets the network redirect the
                // browser to its login page. The Location header is never
                // followed here -- the browser decides.
                let _ = std::process::Command::new("xdg-open")
                    .arg("http://ping.archlinux.org/nm-check.txt")
                    .spawn();
            }
        }

        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let button = match self.bar_label() {
            None => self
                .core
                .applet
                .icon_button(&self.icon_name)
                .on_press_down(Message::TogglePopup),

            Some(label) => {
                // `icon_button` is a fixed square, so the labelled form is built
                // by hand: an invisible vertical spacer holds the panel's
                // suggested height, and `applet.text` picks up the bar's font
                // size, matching how the stock clock applet lays itself out.
                let suggested = self.core.applet.suggested_size(true);
                let padding = self.core.applet.suggested_padding(true);
                let height = f32::from(suggested.1 + 2 * padding.1);

                let content = row::with_children(vec![
                    from_name(&*self.icon_name)
                        .size(suggested.0)
                        .symbolic(true)
                        .into(),
                    self.core.applet.text(label).into(),
                    container(space::vertical().height(Length::Fixed(height))).into(),
                ])
                .spacing(4)
                .align_y(Alignment::Center);

                button::custom(content)
                    .padding([0, padding.0])
                    .on_press_down(Message::TogglePopup)
                    .class(cosmic::theme::Button::AppletIcon)
            }
        };

        autosize::autosize(button, AUTOSIZE_MAIN_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let mut content = column::with_capacity(12).spacing(12).padding([8, 12]);

        content = content.push(self.hero());

        if self.restricted() {
            content = content.push(
                button::suggested(fl!("open-captive-portal"))
                    .width(Length::Fill)
                    .on_press(Message::OpenCaptivePortal),
            );
        }

        if let Some(speedtest) = self.speedtest_row() {
            content = content.push(speedtest);
        }

        if self.status.connected() {
            content = content.push(divider::horizontal::default());
            content = content.push(self.metrics());
        }

        if let Some(qr) = &self.qr {
            content = content.push(divider::horizontal::default());
            content = content.push(self.qr_panel(qr));

            return self
                .core
                .applet
                .popup_container(container(content).width(Length::Fixed(POPUP_WIDTH)))
                .into();
        }

        if let Some(band) = self.band_section() {
            content = content.push(divider::horizontal::default());
            content = content.push(band);
        }

        if self.status.connected() {
            content = content.push(divider::horizontal::default());
            content = content.push(self.dns_section());
        }

        content = content.push(divider::horizontal::default());
        content = content.push(self.network_list());

        content = content.push(divider::horizontal::default());
        content = content.push(self.settings_section());

        self.core
            .applet
            .popup_container(container(content).width(Length::Fixed(POPUP_WIDTH)))
            .into()
    }
}
