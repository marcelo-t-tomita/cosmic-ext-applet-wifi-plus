// SPDX-License-Identifier: MIT

mod app;
mod config;
mod localize;
mod model;
mod net;

/// `--status` prints one round of every reading the panel makes and exits.
/// Useful for checking the data layer on a machine without opening the popup.
fn dump_latency(sample: Option<f64>) -> String {
    match sample {
        Some(ms) => format!("{} ms", model::format_latency_value(ms)),
        None => "timeout".to_string(),
    }
}

fn dump_status() {
    let status = net::status();
    let (router, internet) = net::pings(&status.gateway);
    let band = net::band_status(&status);

    println!("kind          {:?}", status.kind);
    println!("iface         {}", status.iface);
    println!("ssid          {}", status.ssid);
    println!("signal        {}%", status.signal);
    println!("freq          {} MHz", status.freq_mhz);
    println!("signal_dbm    {}", status.signal_dbm);
    println!("bitrate       {}", status.bitrate);
    println!("ip            {}/{}", status.ip, status.prefix);
    println!("gateway       {}", status.gateway);
    println!(
        "rx/tx         {} / {}",
        status
            .rx_bytes
            .map(|v| model::format_bytes(v as f64))
            .unwrap_or_else(|| "--".into()),
        status
            .tx_bytes
            .map(|v| model::format_bytes(v as f64))
            .unwrap_or_else(|| "--".into())
    );
    println!(
        "ping          router {} / internet {}",
        dump_latency(router),
        dump_latency(internet)
    );
    println!(
        "band          current={} selected={} available={:?} profile={}",
        band.current, band.selected, band.available, band.profile
    );
    println!("dns           {}", net::dns_provider(&band.profile));
    println!("wifi radio    {}", net::wifi_enabled());

    println!("\nnetworks:");
    let rows = net::scan(true);
    for (index, row) in rows.iter().enumerate() {
        if let Some(section) = model::wifi_section(&rows, index) {
            println!("  -- {section:?}");
        }
        println!(
            "  {:>3}%  {:<28} {:?}{}{}",
            row.signal,
            row.ssid,
            row.security,
            if row.known { "  known" } else { "" },
            if row.connected { "  CONNECTED" } else { "" }
        );
    }
}

fn main() -> cosmic::iced::Result {
    if std::env::args().any(|arg| arg == "--status") {
        dump_status();
        return Ok(());
    }

    tracing_subscriber::fmt::init();
    localize::localize();
    app::run()
}
