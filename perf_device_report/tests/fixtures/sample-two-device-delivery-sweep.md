<!--
SYNTHETIC TEST FIXTURE — not a real device capture. Used only to exercise
perf_device_report's multi-block parsing in tests/parse_report.rs (Part G's
"one block per payload size per UI-load state" shape). Never archive this
file's contents as a real MEASURED reading.
-->
```meshcadet-perf-report
kit_version: 1
build_ref: fixture03
capture_date: 2026-08-02
section: two-device-delivery
payload_bytes: 10
ui_load: idle
peer_present: yes
notes: synthetic fixture — control run
--- raw-serial-log ---
RX RxDone: 10 bytes, rssi=-40dBm snr=9dB (raw 1/1)
RX DM from 0x12 ...
ACK received: matches last-sent DM
PERF rx-notice-latency: n=1 min=210 mean=210 max=210 p95=210
PERF ui-starvation: cumulative=0 longest=0 (window=30s)
--- end-raw-serial-log ---
```

```meshcadet-perf-report
kit_version: 1
build_ref: fixture03
capture_date: 2026-08-02
section: two-device-delivery
payload_bytes: 10
ui_load: navigating
peer_present: yes
notes: synthetic fixture — UI-active run, same payload
--- raw-serial-log ---
RX RxDone: 10 bytes, rssi=-41dBm snr=8dB (raw 1/1)
RX DM from 0x12 ...
ACK received: matches last-sent DM
CAD: channel busy, deferring retry 40ms
PERF rx-notice-latency: n=1 min=640 mean=640 max=640 p95=640
PERF ui-starvation: cumulative=180 longest=95 (window=30s)
--- end-raw-serial-log ---
```
