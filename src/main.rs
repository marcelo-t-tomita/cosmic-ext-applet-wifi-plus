// SPDX-License-Identifier: MIT

mod app;
mod model;
mod net;

/// `--status` prints one round of every reading the panel makes and exits.
/// Useful for checking the data layer on a machine without opening the popup.
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
        model::format_ping_latency(router.unwrap_or(-1.0), router.is_some()),
        model::format_ping_latency(internet.unwrap_or(-1.0), internet.is_some())
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
        let section = model::wifi_section_title(&rows, index).unwrap_or("");
        if !section.is_empty() {
            println!("  -- {section}");
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
    app::run()
}
