<!--
SYNTHETIC TEST FIXTURE — not a real device capture. Used only to exercise
perf_device_report's parser in tests/parse_report.rs. Never archive this
file's contents as a real MEASURED reading.
-->
```meshcadet-perf-report
kit_version: 1
build_ref: fixture01
capture_date: 2026-08-02
section: baseline
payload_bytes: n/a
ui_load: n/a
peer_present: no
notes: synthetic fixture — idle window then one navigation window
--- raw-serial-log ---
firmware build: fixture01
identity ready: pub_hash=0x7a, pubkey=aa11bb22cc33dd44ee55ff660011223344556677889900aabbccddeeff0011
PERF phase=gps: n=1 min=900 mean=900 max=900 p95=900
PERF phase=battery: n=1 min=140 mean=140 max=140 p95=140
PERF phase=cad: n=0 min=0 mean=0 max=0 p95=0
PERF phase=tx: n=0 min=0 mean=0 max=0 p95=0
PERF phase=rx_poll: n=30 min=2 mean=3 max=6 p95=5
PERF phase=ui_step: n=118 min=8 mean=40 max=95 p95=70
PERF rx-notice-latency: n=0 min=0 mean=0 max=0 p95=0
PERF ui-starvation: cumulative=0 longest=0 (window=30s)
PERF core-utilization: core0=3.1 core1=n/a
PERF heap-internal: free=184000 min_ever=184000
main-task: stack HWM: 19000 B free / 49152 B total = 30152 B peak (38.6% headroom)
PERF phase=gps: n=1 min=920 mean=920 max=920 p95=920
PERF phase=battery: n=1 min=135 mean=135 max=135 p95=135
PERF phase=cad: n=0 min=0 mean=0 max=0 p95=0
PERF phase=tx: n=0 min=0 mean=0 max=0 p95=0
PERF phase=rx_poll: n=30 min=2 mean=3 max=7 p95=6
PERF phase=ui_step: n=121 min=9 mean=52 max=430 p95=190
PERF rx-notice-latency: n=0 min=0 mean=0 max=0 p95=0
PERF ui-starvation: cumulative=0 longest=0 (window=30s)
PERF core-utilization: core0=4.0 core1=n/a
PERF heap-internal: free=183104 min_ever=182016
main-task: stack HWM: 18900 B free / 49152 B total = 30252 B peak (38.4% headroom)
--- end-raw-serial-log ---
```
