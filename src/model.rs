//! Presentation logic: a port of Omarchy's `Model.js`. Kept free of any
//! libcosmic types so it can be unit-tested on its own.

use std::time::Instant;

/// Named theme icons instead of Omarchy's Nerd Font glyphs -- COSMIC ships the
/// freedesktop symbolic set, which follows the panel theme and scales with it.
pub fn wifi_icon_for(strength: i32) -> &'static str {
    match strength {
        s if s >= 80 => "network-wireless-signal-excellent-symbolic",
        s if s >= 60 => "network-wireless-signal-good-symbolic",
        s if s >= 40 => "network-wireless-signal-ok-symbolic",
        s if s >= 20 => "network-wireless-signal-weak-symbolic",
        _ => "network-wireless-signal-none-symbolic",
    }
}

pub fn connection_icon(kind: crate::net::Kind, signal: i32, restricted: bool) -> &'static str {
    use crate::net::Kind;

    match kind {
        Kind::Wifi if restricted => "network-wireless-no-route-symbolic",
        Kind::Wifi => wifi_icon_for(signal),
        Kind::Ethernet if restricted => "network-wired-no-route-symbolic",
        Kind::Ethernet => "network-wired-symbolic",
        Kind::Disconnected => "network-wireless-offline-symbolic",
    }
}

pub fn format_header_speed(mbps: &str) -> String {
    let value: i64 = mbps.trim().parse().unwrap_or(0);
    if value <= 0 {
        return String::new();
    }

    if value >= 1000 {
        let gbit = value as f64 / 1000.0;
        if value % 1000 == 0 {
            format!("{gbit:.0}gbit")
        } else {
            format!("{gbit:.1}gbit")
        }
    } else {
        format!("{value}mbit")
    }
}


pub fn band_label(band: &str) -> String {
    match band {
        "auto" => "Auto".to_string(),
        "" => String::new(),
        other => format!("{other}ghz"),
    }
}

/// Under Automatic the pills are hidden, so the header carries the live band
/// instead -- "WI-FI BAND: 2.4GHZ". Once a band is pinned the pills are on
/// screen and say it themselves, so the header drops back to a plain label.
pub fn band_section_title(selected: &str, current: &str) -> String {
    if selected != "auto" {
        return "WI-FI BAND".to_string();
    }

    let label = band_label(current);
    if label.is_empty() {
        return "WI-FI BAND".to_string();
    }

    format!("WI-FI BAND: {}", label.to_uppercase())
}


pub fn format_bytes(bytes: f64) -> String {
    let n = if bytes.is_finite() && bytes >= 0.0 { bytes } else { 0.0 };

    if n < 1024.0 {
        format!("{} B", n.round())
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else if n < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", n / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn format_rate(bytes_per_sec: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// `has_samples` false means no probe has come back yet, which is different
/// from a probe that timed out. The rows stay mounted through that gap and read
/// "--" so the grid doesn't reflow a second after the panel opens.
pub fn format_ping_latency(ms: f64, has_samples: bool) -> String {
    if !has_samples {
        return "--".to_string();
    }
    if !ms.is_finite() || ms < 0.0 {
        return "Timeout".to_string();
    }

    if ms > 0.0 && ms < 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{ms:.0} ms")
    }
}

pub fn format_packet_loss(percent: i32, has_samples: bool) -> String {
    if !has_samples {
        return "--".to_string();
    }
    if percent <= 0 {
        return "0%".to_string();
    }
    format!("{percent}%")
}

pub const CONNECTION_PHRASES: [&str; 7] = [
    "Wiring bits",
    "Handling packets",
    "Sorting frames",
    "Hauling bytes",
    "Routing crumbs",
    "Counting collisions",
    "Bending light",
];

/// Rolling byte counters turned into a live rate. A change of interface resets
/// the baseline instead of reporting the difference between two unrelated
/// counters as a huge spike.
#[derive(Debug, Clone, Default)]
pub struct Throughput {
    iface: String,
    prev_rx: u64,
    prev_tx: u64,
    prev_at: Option<Instant>,
    pub download_rate: f64,
    pub upload_rate: f64,
}

impl Throughput {
    pub fn sample(&mut self, iface: &str, rx: u64, tx: u64) {
        let now = Instant::now();

        if iface != self.iface || self.prev_at.is_none() {
            self.iface = iface.to_string();
            self.prev_rx = rx;
            self.prev_tx = tx;
            self.prev_at = Some(now);
            self.download_rate = 0.0;
            self.upload_rate = 0.0;
            return;
        }

        let elapsed = now.duration_since(self.prev_at.unwrap()).as_secs_f64();
        if elapsed > 0.0 {
            self.download_rate = rx.saturating_sub(self.prev_rx) as f64 / elapsed;
            self.upload_rate = tx.saturating_sub(self.prev_tx) as f64 / elapsed;
        }

        self.prev_rx = rx;
        self.prev_tx = tx;
        self.prev_at = Some(now);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

const PING_WINDOW: usize = 5;

/// A short ring of probe results. `None` is a timeout, which counts toward
/// packet loss; the average ignores them.
#[derive(Debug, Clone, Default)]
pub struct PingSamples {
    iface: String,
    router: Vec<Option<f64>>,
    internet: Vec<Option<f64>>,
}

impl PingSamples {
    pub fn sample(&mut self, iface: &str, router: Option<f64>, internet: Option<f64>) {
        if iface != self.iface {
            self.iface = iface.to_string();
            self.router.clear();
            self.internet.clear();
        }

        push_capped(&mut self.router, router);
        push_capped(&mut self.internet, internet);
    }

    pub fn has_internet_samples(&self) -> bool {
        !self.internet.is_empty()
    }

    pub fn internet_latency(&self) -> f64 {
        average(&self.internet)
    }

    pub fn internet_packet_loss(&self) -> i32 {
        packet_loss(&self.internet)
    }

}

fn push_capped(samples: &mut Vec<Option<f64>>, value: Option<f64>) {
    samples.push(value);
    while samples.len() > PING_WINDOW {
        samples.remove(0);
    }
}

fn average(samples: &[Option<f64>]) -> f64 {
    let values: Vec<f64> = samples.iter().filter_map(|s| *s).collect();
    if values.is_empty() {
        return -1.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn packet_loss(samples: &[Option<f64>]) -> i32 {
    if samples.is_empty() {
        return 0;
    }
    let lost = samples.iter().filter(|s| s.is_none()).count();
    ((lost as f64 / samples.len() as f64) * 100.0).round() as i32
}

/// "KNOWN NETWORKS" above the saved ones, "OTHER NETWORKS" at the first unsaved
/// row, nothing anywhere else.
pub fn wifi_section_title(rows: &[crate::net::WifiRow], index: usize) -> Option<&'static str> {
    let row = rows.get(index)?;

    if row.known && index == 0 {
        return Some("KNOWN NETWORKS");
    }
    if !row.known && (index == 0 || rows.get(index - 1).map(|p| p.known).unwrap_or(false)) {
        return Some("OTHER NETWORKS");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_transfer_units() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(2048.0), "2.0 KB");
        assert_eq!(format_rate(1_572_864.0), "1.5 MB/s");
        assert_eq!(format_bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3.00 GB");
    }

    #[test]
    fn formats_latency_with_sub_ten_precision() {
        assert_eq!(format_ping_latency(0.0, false), "--");
        assert_eq!(format_ping_latency(-1.0, true), "Timeout");
        assert_eq!(format_ping_latency(4.23, true), "4.2 ms");
        assert_eq!(format_ping_latency(42.7, true), "43 ms");
    }

    #[test]
    fn band_header_carries_live_band_only_under_auto() {
        assert_eq!(band_section_title("auto", "5"), "WI-FI BAND: 5GHZ");
        assert_eq!(band_section_title("5", "5"), "WI-FI BAND");
        assert_eq!(band_section_title("auto", ""), "WI-FI BAND");
    }

    #[test]
    fn ethernet_speed_reads_as_gbit_above_a_thousand() {
        assert_eq!(format_header_speed("1000"), "1gbit");
        assert_eq!(format_header_speed("2500"), "2.5gbit");
        assert_eq!(format_header_speed("100"), "100mbit");
        assert_eq!(format_header_speed("-1"), "");
    }

    #[test]
    fn packet_loss_counts_timeouts_but_average_ignores_them() {
        let mut samples = PingSamples::default();
        samples.sample("wlp2s0", Some(1.0), Some(10.0));
        samples.sample("wlp2s0", Some(1.0), None);
        samples.sample("wlp2s0", Some(1.0), Some(20.0));

        assert_eq!(samples.internet_latency(), 15.0);
        assert_eq!(samples.internet_packet_loss(), 33);
    }

    #[test]
    fn switching_interface_resets_the_throughput_baseline() {
        let mut throughput = Throughput::default();
        throughput.sample("wlp2s0", 1_000_000, 500_000);
        throughput.sample("enp0s1", 10, 10);

        assert_eq!(throughput.download_rate, 0.0);
    }
}
