<!--
SYNTHETIC TEST FIXTURE — not a real device capture. Used only to exercise
perf_device_report's calibration wiring in tests/parse_report.rs. Never
archive this file's contents as a real MEASURED reading.
-->
```meshcadet-perf-report
kit_version: 1
build_ref: fixture02
capture_date: 2026-08-02
section: calibration
payload_bytes: n/a
ui_load: navigating
peer_present: no
notes: synthetic fixture — 5 dummy-contact DMs sent during capture
--- raw-serial-log ---
firmware build: fixture02
identity ready: pub_hash=0x7b, pubkey=aa11bb22cc33dd44ee55ff660011223344556677889900aabbccddeeff0022
PERF phase=gps: n=1 min=910 mean=910 max=910 p95=910
PERF phase=battery: n=1 min=138 mean=138 max=138 p95=138
PERF phase=cad: n=6 min=8600 mean=9350 max=10200 p95=9950
PERF phase=tx: n=6 min=83 mean=180 max=310 p95=290
PERF phase=rx_poll: n=30 min=2 mean=3 max=6 p95=5
PERF phase=ui_step: n=140 min=10 mean=60 max=520 p95=210
TX: 12 bytes, 83ms airtime
TX: 12 bytes, 83ms airtime
--- end-raw-serial-log ---
```
