# cosmic-ext-applet-wifi-plus

An [Omarchy](https://omarchy.org)-style Wi-Fi panel for the COSMIC desktop —
live link metrics, band pinning and DNS switching in the panel popup.

Omarchy's panel is a Quickshell/QML plugin that only runs under its own shell.
Its *data* layer, though, is plain `nmcli` / `iw` / `ping` / `/sys/class/net`,
which is portable anywhere NetworkManager runs. This reimplements the panel as a
native COSMIC applet: the readings and layout follow Omarchy, the UI is
libcosmic so it docks into `cosmic-panel` and follows the COSMIC theme.

## What the panel shows

**Hero** — signal icon, SSID (Ethernet shows its negotiated speed, e.g.
`Ethernet (2.5gbit)`), and a status line: `NOT CONNECTED`,
`LIMITED INTERNET ACCESS`, or one of Omarchy's rotating phrases
(*Hauling bytes*, *Bending light*, …). Alongside: QR share, speed test, and the
Wi-Fi radio toggle.

**Metrics** — Ping and Packet Loss (five-sample rolling window against
`1.1.1.1`), Receiving/Sending rates, Downloaded/Uploaded totals, IP Address and
Gateway. Unsampled fields read `--` rather than popping in late.

**Wi-Fi band** — appears only when the AP answers on more than one band. An
`AUTOMATIC` toggle plus 2.4/5/6 GHz pills, pinned through
`802-11-wireless.band`. A band that fails to associate is rolled back rather
than leaving the machine offline.

**DNS provider** — DHCP / Cloudflare / Google pills (`Custom` is reported, not
settable, when servers were configured elsewhere).

**Networks** — grouped `KNOWN NETWORKS` / `OTHER NETWORKS`, sorted
connected → known → signal, with connect, disconnect, forget, and inline
passphrase entry (plus an identity field on WPA-Enterprise).

**Show network name in panel** — a toggle at the foot of the popup puts the
connected SSID next to the bar icon. On by default, persisted through
cosmic-config. The label is clipped at 18 characters, and is dropped entirely on
a vertical panel, on Ethernet, and when offline — cases where it would only cost
width.

## Build and install

```sh
cargo build --release
./install.sh      # installs to ~/.local/bin, registers with cosmic-panel
```

`install.sh` backs up `plugins_wings` before editing it and restarts
`cosmic-panel`. `./uninstall.sh` reverses all of it.

The stock COSMIC network applet is left in place; remove it in
*Settings → Desktop → Panel* if you'd rather have just this one.

### Optional runtime dependencies

- `iw` — RSSI in dBm and the negotiated TX bitrate. Everything else, the band
  selector included, works without it: nmcli reports the frequency too, just
  without those two readings.
- `curl` — required for the fast.com speed test.

```sh
sudo apt install iw
```

## Translations

English and Brazilian Portuguese ship in `i18n/`, loaded with
[i18n-embed](https://github.com/kellpossible/cargo-i18n) + Fluent, the same
setup the stock COSMIC applets use. The applet follows the desktop locale and
falls back to English. Adding a language means dropping one `.ftl` file into
`i18n/<locale>/` — no Rust changes.

## Checking the data layer

`cosmic-ext-applet-wifi-plus --status` prints one round of every reading the panel
makes and exits — handy for confirming the nmcli parsing on a given machine
without opening the popup.

## Notes

- Saved profiles are matched to scan results by name. NetworkManager allows a
  profile whose SSID differs from its name; such a profile shows as unknown.
- The speed test measures the interface's own byte counters over a five-second
  window, so parallel streams add up (same method Omarchy uses).
- While the popup is closed the applet polls every 8s and never forces a scan;
  open, it polls every 2s.

## Credits and license

The panel design and the data-collection logic are ported from
[Omarchy](https://github.com/omacom/omarchy) — `shell/plugins/panels/network`
and `bin/omarchy-network-*`. Omarchy is MIT-licensed; see [NOTICE](NOTICE),
which reproduces its copyright notice in full.

This applet is MIT-licensed too — see [LICENSE](LICENSE). It builds on
[libcosmic](https://github.com/pop-os/libcosmic) (MPL-2.0).

## Status

Built and running on Pop!_OS 24.04 (COSMIC epoch 1.0, `cosmic-applets` 1.0.15).
`libcosmic` is pinned to the exact revision that Pop packages its own applets
from, so the applet matches the stack already on the system. Other COSMIC
builds may need that pin moved.
