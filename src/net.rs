//! Data layer: a Rust port of `omarchy-network-status`, `omarchy-network-band`
//! and `omarchy-network-qr`. Everything is read through `nmcli`, `ip`, `iw`,
//! `ping` and `/sys/class/net`, so nothing here is Arch- or Hyprland-specific.
//!
//! Every call is blocking and short-lived; the UI runs them off the render
//! thread via `Task::perform`.

use std::process::Command;

pub const INTERNET_PROBE: &str = "1.1.1.1";

/// nmcli translates state words such as "connected", which would silently stop
/// matching under this machine's pt_BR session, so every invocation is pinned
/// to the C locale.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).env("LC_ALL", "C").output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_ok(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn has_command(cmd: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn read_sys(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// `nmcli -t` escapes `:` inside values as `\:`, so a plain `split(':')` would
/// tear an SSID in half. This splits on unescaped colons only and unescapes the
/// rest.
fn split_terse(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ':' {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    fields.push(current);
    fields
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    Wifi,
    Ethernet,
    #[default]
    Disconnected,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub kind: Kind,
    pub iface: String,
    pub ssid: String,
    pub signal: i32,
    pub freq_mhz: f64,
    pub ip: String,
    pub prefix: String,
    pub gateway: String,
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub bitrate: String,
    pub signal_dbm: String,
    pub eth_speed: String,
}

impl Status {
    pub fn connected(&self) -> bool {
        self.kind != Kind::Disconnected
    }
}

/// The default route to the internet probe is what decides which interface is
/// "the" connection -- the same rule the Omarchy script uses, so a machine with
/// both Wi-Fi and Ethernet up reports whichever one actually carries traffic.
fn default_route() -> Option<(String, String, String)> {
    let json = run("ip", &["-j", "route", "get", INTERNET_PROBE])?;
    let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
    let first = parsed.get(0)?;

    let dev = first.get("dev")?.as_str()?.to_string();
    let gateway = first
        .get("gateway")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let src = first
        .get("prefsrc")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some((dev, gateway, src))
}

fn prefix_len(iface: &str) -> String {
    let Some(json) = run("ip", &["-j", "addr", "show", iface]) else {
        return String::new();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) else {
        return String::new();
    };

    parsed
        .get(0)
        .and_then(|v| v.get("addr_info"))
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|a| a.get("family").and_then(|f| f.as_str()) == Some("inet"))
                .and_then(|a| a.get("prefixlen"))
                .and_then(|p| p.as_u64())
        })
        .map(|p| p.to_string())
        .unwrap_or_default()
}

fn is_wireless(iface: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{iface}/wireless")).is_dir()
}

/// `iw dev <iface> link` carries the live association: SSID, RSSI in dBm, the
/// exact frequency and the negotiated TX bitrate. It is optional -- without the
/// `iw` package the panel falls back to nmcli, which knows the SSID and a
/// percentage signal but not dBm or bitrate.
fn iw_link(iface: &str, status: &mut Status) -> bool {
    if !has_command("iw") {
        return false;
    }
    let Some(link) = run("iw", &["dev", iface, "link"]) else {
        return false;
    };
    if link.trim().is_empty() || link.contains("Not connected") {
        return false;
    }

    for line in link.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("SSID: ") {
            status.ssid = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("signal: ") {
            status.signal_dbm = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = line.strip_prefix("freq: ") {
            status.freq_mhz = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .parse()
                .unwrap_or(0.0);
        } else if let Some(rest) = line.strip_prefix("tx bitrate: ") {
            let mut parts = rest.split_whitespace();
            let value = parts.next().unwrap_or("");
            let unit = parts.next().unwrap_or("");
            status.bitrate = format!("{value} {unit}").trim().to_string();
        }
    }

    !status.ssid.is_empty()
}

/// Percentage signal and, when `iw` is missing, the SSID and frequency of the
/// in-use access point.
fn nmcli_wifi_fallback(iface: &str, status: &mut Status) {
    let Some(list) = run(
        "nmcli",
        &[
            "-t",
            "-f",
            "IN-USE,SIGNAL,FREQ,SSID",
            "dev",
            "wifi",
            "list",
            "ifname",
            iface,
            "--rescan",
            "no",
        ],
    ) else {
        return;
    };

    for line in list.lines() {
        let fields = split_terse(line);
        if fields.first().map(String::as_str) != Some("*") {
            continue;
        }

        status.signal = fields.get(1).and_then(|s| s.parse().ok()).unwrap_or(-1);

        if status.freq_mhz == 0.0 {
            // nmcli prints "5180 MHz"; keep the leading digits.
            status.freq_mhz = fields
                .get(2)
                .map(|f| f.trim_end_matches(|c: char| !c.is_ascii_digit()))
                .and_then(|f| f.split_whitespace().next())
                .and_then(|f| f.parse().ok())
                .unwrap_or(0.0);
        }

        if status.ssid.is_empty() {
            // The SSID is queried last so one containing ':' reassembles verbatim.
            status.ssid = fields[3..].join(":");
        }
        break;
    }
}

pub fn status() -> Status {
    let mut status = Status::default();

    let Some((iface, gateway, src)) = default_route() else {
        return status;
    };

    status.iface = iface.clone();
    status.gateway = gateway;
    status.ip = src;
    status.prefix = prefix_len(&iface);
    status.rx_bytes = read_sys(&format!("/sys/class/net/{iface}/statistics/rx_bytes"))
        .and_then(|v| v.parse().ok());
    status.tx_bytes = read_sys(&format!("/sys/class/net/{iface}/statistics/tx_bytes"))
        .and_then(|v| v.parse().ok());

    if is_wireless(&iface) {
        status.kind = Kind::Wifi;
        iw_link(&iface, &mut status);
        nmcli_wifi_fallback(&iface, &mut status);
        if status.ssid.is_empty() {
            status.ssid = iface.clone();
        }
    } else {
        status.kind = Kind::Ethernet;
        status.eth_speed = read_sys(&format!("/sys/class/net/{iface}/speed")).unwrap_or_default();
    }

    status
}

/// One ICMP echo with a one second deadline. `None` means the probe timed out,
/// which the panel renders as "Timeout" and counts as a lost packet -- distinct
/// from "no sample yet", which reads "--".
pub fn ping(host: &str) -> Option<f64> {
    let out = run("ping", &["-n", "-c", "1", "-W", "1", host])?;

    out.lines()
        .find_map(|line| line.split("time=").nth(1).or_else(|| line.split("time<").nth(1)))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

pub fn pings(gateway: &str) -> (Option<f64>, Option<f64>) {
    if !has_command("ping") {
        return (None, None);
    }
    let router = if gateway.is_empty() { None } else { ping(gateway) };
    (router, ping(INTERNET_PROBE))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,
    /// Enhanced Open: encrypted but with no credentials to collect, so it gets
    /// neither a padlock nor a passphrase prompt.
    Owe,
    Psk,
    Enterprise,
}

impl Security {
    pub fn requires_credentials(self) -> bool {
        !matches!(self, Security::Open | Security::Owe)
    }
}

#[derive(Debug, Clone)]
pub struct WifiRow {
    pub ssid: String,
    pub signal: i32,
    pub security: Security,
    pub connected: bool,
    pub known: bool,
}

fn parse_security(raw: &str) -> Security {
    let raw = raw.trim();
    if raw.is_empty() || raw == "--" {
        Security::Open
    } else if raw.contains("802.1X") {
        Security::Enterprise
    } else if raw.contains("OWE") {
        Security::Owe
    } else {
        Security::Psk
    }
}

/// Saved Wi-Fi profiles, keyed by the profile name. NetworkManager lets a
/// profile carry an SSID different from its name, but nmcli cannot print both
/// in one call, so the panel matches on the name -- correct for every profile
/// created through the panel itself or through nmcli's own defaults.
fn known_profiles() -> Vec<String> {
    let Some(out) = run("nmcli", &["-t", "-f", "NAME,TYPE", "connection", "show"]) else {
        return Vec::new();
    };

    out.lines()
        .filter_map(|line| {
            let fields = split_terse(line);
            let kind = fields.last()?;
            if kind != "802-11-wireless" {
                return None;
            }
            Some(fields[..fields.len() - 1].join(":"))
        })
        .collect()
}

pub fn scan(rescan: bool) -> Vec<WifiRow> {
    let known = known_profiles();
    let rescan = if rescan { "yes" } else { "no" };

    let Some(out) = run(
        "nmcli",
        &[
            "-t",
            "-f",
            "IN-USE,SIGNAL,SECURITY,SSID",
            "dev",
            "wifi",
            "list",
            "--rescan",
            rescan,
        ],
    ) else {
        return Vec::new();
    };

    let mut rows: Vec<WifiRow> = Vec::new();

    for line in out.lines() {
        let fields = split_terse(line);
        if fields.len() < 4 {
            continue;
        }

        // SSID last again: everything after the third field is part of the name.
        let ssid = fields[3..].join(":");
        if ssid.is_empty() {
            continue;
        }

        let row = WifiRow {
            connected: fields[0] == "*",
            signal: fields[1].parse().unwrap_or(0),
            security: parse_security(&fields[2]),
            known: known.iter().any(|k| k == &ssid),
            ssid,
        };

        // An SSID answering on several bands shows up once per BSS. Keep the
        // strongest sighting so the list is one row per network.
        match rows.iter_mut().find(|existing| existing.ssid == row.ssid) {
            Some(existing) => {
                existing.connected |= row.connected;
                existing.signal = existing.signal.max(row.signal);
            }
            None => rows.push(row),
        }
    }

    rows.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.known.cmp(&a.known))
            .then(b.signal.cmp(&a.signal))
    });

    rows
}

pub fn wifi_enabled() -> bool {
    run("nmcli", &["radio", "wifi"])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false)
}

pub fn set_wifi_enabled(enabled: bool) -> bool {
    run_ok("nmcli", &["radio", "wifi", if enabled { "on" } else { "off" }])
}

pub fn connect_known(ssid: &str) -> Result<(), String> {
    run("nmcli", &["connection", "up", "id", ssid])
        .map(|_| ())
        .ok_or_else(|| "Failed to connect".to_string())
}

/// An open or OWE network takes no credentials, so `password` must be absent
/// from the command line rather than empty -- nmcli rejects an empty one.
pub fn connect_open(ssid: &str) -> Result<(), String> {
    let out = Command::new("nmcli")
        .args(["device", "wifi", "connect", ssid])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| e.to_string())?;

    if out.status.success() {
        Ok(())
    } else {
        Err(failure_reason(&String::from_utf8_lossy(&out.stderr)))
    }
}

pub fn connect_psk(ssid: &str, password: &str) -> Result<(), String> {
    // The passphrase is an argument here, which /proc exposes for the lifetime
    // of the process. nmcli offers no stdin path for `device wifi connect`, so
    // this mirrors what `nmcli` users type anyway; the enterprise path below
    // does use stdin, where a longer-lived profile is being written.
    let out = Command::new("nmcli")
        .args(["device", "wifi", "connect", ssid, "password", password])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| e.to_string())?;

    if out.status.success() {
        Ok(())
    } else {
        Err(failure_reason(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// WPA-Enterprise. The password arrives on stdin and reaches nmcli through the
/// scriptable `connection edit` editor -- argv is world-readable in /proc, so
/// the secret must never be an argument. Ported from Omarchy's
/// `enterpriseConnectScript`.
pub fn connect_enterprise(ssid: &str, identity: &str, password: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;

    let script = r#"u=$(uuidgen); IFS= read -r pw;
 nmcli connection add type wifi con-name "$1" ssid "$1" connection.uuid "$u" \
  wifi-sec.key-mgmt wpa-eap 802-1x.eap peap 802-1x.phase2-auth mschapv2 \
  802-1x.identity "$2" 802-1x.auth-timeout 8 >/dev/null \
 && printf 'set 802-1x.password %s\nsave\nquit\n' "$pw" | nmcli connection edit uuid "$u" >/dev/null \
 && nmcli connection up uuid "$u" \
 || { nmcli connection delete uuid "$u" >/dev/null 2>&1; false; }"#;

    let mut child = Command::new("bash")
        .args(["-c", script, "bash", ssid, identity])
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    child
        .stdin
        .as_mut()
        .ok_or("no stdin")?
        .write_all(format!("{password}\n").as_bytes())
        .map_err(|e| e.to_string())?;

    let out = child.wait_with_output().map_err(|e| e.to_string())?;

    if out.status.success() {
        Ok(())
    } else {
        Err(failure_reason(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// nmcli's stderr mapped onto the same short phrases the Omarchy panel shows.
fn failure_reason(stderr: &str) -> String {
    let lower = stderr.to_lowercase();

    if lower.contains("secrets were required") || lower.contains("no secrets") {
        "Passphrase required".to_string()
    } else if lower.contains("802.1x supplicant") || lower.contains("timeout") {
        "Wrong password".to_string()
    } else if lower.contains("no network with ssid") {
        "Network lost".to_string()
    } else {
        "Failed to connect".to_string()
    }
}

pub fn disconnect(ssid: &str) -> Result<(), String> {
    run("nmcli", &["connection", "down", "id", ssid])
        .map(|_| ())
        .ok_or_else(|| "Failed to disconnect".to_string())
}

pub fn forget(ssid: &str) -> Result<(), String> {
    run("nmcli", &["connection", "delete", "id", ssid])
        .map(|_| ())
        .ok_or_else(|| "Failed to forget".to_string())
}

// ---------------------------------------------------------------------------
// Wi-Fi band -- port of omarchy-network-band
// ---------------------------------------------------------------------------

/// NetworkManager 1.44+ accepts a third band value, so 5GHz and 6GHz can be
/// pinned apart. Pinning the band rather than a BSSID keeps working when the AP
/// rotates BSSIDs, and leaves roaming between APs intact.
fn nm_band_for(band: &str) -> Option<&'static str> {
    match band {
        "2.4" => Some("bg"),
        "5" => Some("a"),
        "6" => Some("6GHz"),
        _ => None,
    }
}

fn band_from_nm(raw: &str) -> String {
    match raw.trim() {
        "bg" => "2.4",
        "a" => "5",
        "6GHz" => "6",
        _ => "auto",
    }
    .to_string()
}

/// The boundaries mirror `format_header_freq` so the panel's label and the band
/// selector can never disagree.
pub fn band_for_freq(mhz: f64) -> Option<&'static str> {
    if (2400.0..2500.0).contains(&mhz) {
        Some("2.4")
    } else if (4900.0..5925.0).contains(&mhz) {
        Some("5")
    } else if (5925.0..7125.0).contains(&mhz) {
        Some("6")
    } else {
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct BandStatus {
    /// The band the radio is actually on.
    pub current: String,
    /// The pinned choice, "auto" when nothing is pinned.
    pub selected: String,
    pub available: Vec<String>,
    pub profile: String,
}

fn wifi_profile(iface: &str) -> String {
    run(
        "nmcli",
        &["-e", "no", "-g", "GENERAL.CONNECTION", "device", "show", iface],
    )
    .map(|s| s.trim().to_string())
    .unwrap_or_default()
}

/// Every band the SSID is reachable on, low to high, always including the one
/// already in use -- a weak radio gets missed by plenty of scans, and the band
/// we are sitting on must never be absent from its own list of options. A band
/// the AP does not answer on is never offered: pinning to it would drop the
/// connection with nothing to reassociate to.
fn available_bands(iface: &str, ssid: &str, current: &str) -> Vec<String> {
    let mut bands: Vec<String> = Vec::new();
    if !current.is_empty() {
        bands.push(current.to_string());
    }

    // --rescan no reads NetworkManager's cache, which the panel's own scanner
    // keeps warm; forcing a scan here would stall every poll.
    if let Some(out) = run(
        "nmcli",
        &[
            "-t", "-f", "FREQ,SSID", "dev", "wifi", "list", "ifname", iface, "--rescan", "no",
        ],
    ) {
        for line in out.lines() {
            let fields = split_terse(line);
            if fields.len() < 2 {
                continue;
            }
            if fields[1..].join(":") != ssid {
                continue;
            }
            let mhz: f64 = fields[0]
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            if let Some(band) = band_for_freq(mhz) {
                bands.push(band.to_string());
            }
        }
    }

    bands.sort_by(|a, b| {
        a.parse::<f64>()
            .unwrap_or(0.0)
            .partial_cmp(&b.parse::<f64>().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    bands.dedup();
    bands
}

pub fn band_status(status: &Status) -> BandStatus {
    if status.kind != Kind::Wifi || status.ssid.is_empty() {
        return BandStatus::default();
    }

    let current = band_for_freq(status.freq_mhz).unwrap_or("").to_string();
    let profile = wifi_profile(&status.iface);
    let selected = if profile.is_empty() {
        "auto".to_string()
    } else {
        band_from_nm(
            &run(
                "nmcli",
                &["-e", "no", "-g", "802-11-wireless.band", "connection", "show", &profile],
            )
            .unwrap_or_default(),
        )
    };

    BandStatus {
        available: available_bands(&status.iface, &status.ssid, &current),
        current,
        selected,
        profile,
    }
}

/// A band change only takes effect on reassociation. If the radio cannot come
/// back up on the requested band, the previous setting is put back and
/// reconnected rather than leaving the machine stranded offline.
pub fn set_band(profile: &str, target: &str) -> Result<(), String> {
    if profile.is_empty() {
        return Err("No active Wi-Fi connection profile".to_string());
    }

    let desired = if target == "auto" {
        ""
    } else {
        nm_band_for(target).ok_or_else(|| format!("Unknown band {target}"))?
    };

    let previous = run(
        "nmcli",
        &["-e", "no", "-g", "802-11-wireless.band", "connection", "show", profile],
    )
    .unwrap_or_default()
    .trim()
    .to_string();

    if previous == desired {
        return Ok(());
    }

    if !run_ok(
        "nmcli",
        &["connection", "modify", profile, "802-11-wireless.band", desired],
    ) {
        return Err("Could not set band".to_string());
    }

    if run_ok("nmcli", &["connection", "up", profile]) {
        return Ok(());
    }

    let _ = run_ok(
        "nmcli",
        &["connection", "modify", profile, "802-11-wireless.band", &previous],
    );
    let _ = run_ok("nmcli", &["connection", "up", profile]);

    Err(format!("Could not connect on {target}GHz; reverted"))
}

// ---------------------------------------------------------------------------
// DNS
// ---------------------------------------------------------------------------

pub const DNS_CLOUDFLARE: &str = "1.1.1.1,1.0.0.1";
pub const DNS_GOOGLE: &str = "8.8.8.8,8.8.4.4";

pub fn dns_provider(profile: &str) -> String {
    if profile.is_empty() {
        return "DHCP".to_string();
    }

    let servers = run(
        "nmcli",
        &["-e", "no", "-g", "ipv4.dns", "connection", "show", profile],
    )
    .unwrap_or_default()
    .trim()
    .to_string();

    match servers.as_str() {
        "" => "DHCP".to_string(),
        DNS_CLOUDFLARE => "Cloudflare".to_string(),
        DNS_GOOGLE => "Google".to_string(),
        _ => "Custom".to_string(),
    }
}

/// Writing `ipv4.dns` only takes effect on reactivation, so the profile is
/// bounced the same way a band change is.
pub fn set_dns(profile: &str, servers: &str) -> Result<(), String> {
    if profile.is_empty() {
        return Err("No active connection profile".to_string());
    }

    let ignore_auto = if servers.is_empty() { "no" } else { "yes" };

    if !run_ok("nmcli", &["connection", "modify", profile, "ipv4.dns", servers])
        || !run_ok(
            "nmcli",
            &["connection", "modify", profile, "ipv4.ignore-auto-dns", ignore_auto],
        )
    {
        return Err("Could not set DNS".to_string());
    }

    if run_ok("nmcli", &["connection", "up", profile]) {
        Ok(())
    } else {
        Err("Could not reapply the connection".to_string())
    }
}

// ---------------------------------------------------------------------------
// QR sharing -- port of omarchy-network-qr
// ---------------------------------------------------------------------------

/// The `WIFI:` payload a phone camera understands. Special characters are
/// escaped per the Wi-Fi Alliance's QR format.
pub fn wifi_qr_payload(iface: &str) -> Option<String> {
    let uuid = run(
        "nmcli",
        &["--get-values", "GENERAL.CON-UUID", "device", "show", iface],
    )?
    .lines()
    .next()?
    .trim()
    .to_string();

    if uuid.is_empty() {
        return None;
    }

    let fields = run(
        "nmcli",
        &[
            "--show-secrets",
            "--escape",
            "no",
            "--get-values",
            "802-11-wireless.ssid,802-11-wireless-security.key-mgmt,802-11-wireless-security.psk",
            "connection",
            "show",
            &uuid,
        ],
    )?;

    let mut lines = fields.lines();
    let ssid = lines.next()?.trim().to_string();
    let key_mgmt = lines.next().unwrap_or("").trim().to_string();
    let psk = lines.next().unwrap_or("").trim().to_string();

    if ssid.is_empty() {
        return None;
    }

    let escape = |value: &str| {
        value
            .replace('\\', "\\\\")
            .replace(';', "\\;")
            .replace(',', "\\,")
            .replace(':', "\\:")
            .replace('"', "\\\"")
    };

    let auth = if key_mgmt.is_empty() || key_mgmt == "none" {
        "nopass"
    } else {
        "WPA"
    };

    Some(if auth == "nopass" {
        format!("WIFI:T:nopass;S:{};;", escape(&ssid))
    } else {
        format!("WIFI:T:WPA;S:{};P:{};;", escape(&ssid), escape(&psk))
    })
}

// ---------------------------------------------------------------------------
// Speed test -- port of omarchy-network-speedtest
// ---------------------------------------------------------------------------

/// Netflix's fast.com endpoints, measured against this interface's own byte
/// counters rather than curl's reported rate, so parallel streams add up.
pub fn speedtest(iface: &str, upload: bool) -> Result<f64, String> {
    use std::time::{Duration, Instant};

    if !has_command("curl") {
        return Err("curl is required".to_string());
    }

    let token = "YXNkZmFzZGxmbnNkYWZoYXNkZmhrYWxm";
    let api = format!(
        "https://api.fast.com/netflix/speedtest/v2?https=true&token={token}&urlCount=3"
    );

    let body = run("curl", &["-fsS", &api]).ok_or("Failed to reach the speed test API")?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "Bad speed test response".to_string())?;

    let urls: Vec<String> = parsed
        .get("targets")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("url").and_then(|u| u.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if urls.is_empty() {
        return Err("No speed test endpoints".to_string());
    }

    let counter = format!(
        "/sys/class/net/{iface}/statistics/{}_bytes",
        if upload { "tx" } else { "rx" }
    );
    let sample = || -> u64 { read_sys(&counter).and_then(|v| v.parse().ok()).unwrap_or(0) };

    let mut children = Vec::new();
    for url in urls.iter().take(3) {
        for _ in 0..3 {
            let child = if upload {
                Command::new("curl")
                    .args([
                        "-fsS",
                        "-o",
                        "/dev/null",
                        "--max-time",
                        "12",
                        "-X",
                        "POST",
                        "-H",
                        "Content-Type: application/octet-stream",
                        "--data-binary",
                        "@/dev/zero",
                        url,
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
            } else {
                Command::new("curl")
                    .args(["-fsS", "-o", "/dev/null", "--max-time", "12", url])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
            };

            if let Ok(child) = child {
                children.push(child);
            }
        }
    }

    if children.is_empty() {
        return Err("Could not start the speed test".to_string());
    }

    // Let the streams ramp up before the measurement window opens.
    std::thread::sleep(Duration::from_millis(1500));
    let start_bytes = sample();
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(5000));
    let delta = sample().saturating_sub(start_bytes);
    let seconds = start.elapsed().as_secs_f64();

    for mut child in children {
        let _ = child.kill();
        let _ = child.wait();
    }

    if seconds <= 0.0 {
        return Err("Measurement failed".to_string());
    }

    Ok((delta as f64 * 8.0) / seconds / 1_000_000.0)
}
