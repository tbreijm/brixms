# Alpha.9 scale qualification: detailed appendix

[Return to the concise qualification report](alpha9-scale.md).

**QUALIFIED** — the validated summary reports all six size/profile cells complete with at least five valid repetitions, and the ten-revision logical-storage budget passed at every size.

This report is rendered from the validated A9-7 summary. It describes a public synthetic Atlas-shaped fixture only; it is not an Atlas dataset measurement and makes no comparison with Atlas performance figures.

## Build and host identity

| Field | Value |
|---|---|
| Package/build | brix-kb alpha.9 release benchmark example; native Rust |
| Embedded build commit | 12e37f8b40f2137516e6c9ede60c7431d942fdd5 |
| Capture repository commit | 12e37f8b40f2137516e6c9ede60c7431d942fdd5 |
| Benchmark executable SHA-256 | fc33c9f1746551bd5aa50d68e8e1afae4f98c4cdbfed99789e5cb79030d9538e |
| Rust toolchain | rustc 1.96.1 (31fca3adb 2026-06-26) |
| Host | Apple M1 Pro |
| CPU count | 10 |
| Memory | 17,179,869,184 B (16,384.00 MiB) |
| Operating system | macOS 15.6.1 build 24G90 |
| Host metadata SHA-256 | 002cd2937eb2c281286e85a37a83e684a4b7aebd1869badf1cf4b61fd57cca81 |
| Raw capture | docs/performance/raw/alpha9-scale-all.jsonl |

Host metadata: [`alpha9-host.json`](alpha9-host.json). Host metadata was collected outside measured invocations.

## Fixture row counts

| Source facts | Profile | Complete reps | Source relation rows | Derived relation rows |
|---|---|---|---|---|
| 8,000 | single | 5 | events 6,206; orders 800; inventory 64; entity_a 400; entity_b 400; catalogue 128; decision_labels 2 | activity_pages 20; all_entities 400; fulfillment 800; decision_value_join 800 |
| 8,000 | ten | 5 | events 6,206; orders 800; inventory 64; entity_a 400; entity_b 400; catalogue 128; decision_labels 2 | activity_pages 20; all_entities 400; fulfillment 800; decision_value_join 800 |
| 80,000 | single | 5 | events 63,806; orders 8,000; inventory 64; entity_a 4,000; entity_b 4,000; catalogue 128; decision_labels 2 | activity_pages 920; all_entities 4,000; fulfillment 8,000; decision_value_join 8,000 |
| 80,000 | ten | 5 | events 63,806; orders 8,000; inventory 64; entity_a 4,000; entity_b 4,000; catalogue 128; decision_labels 2 | activity_pages 920; all_entities 4,000; fulfillment 8,000; decision_value_join 8,000 |
| 800,000 | single | 5 | events 639,806; orders 80,000; inventory 64; entity_a 40,000; entity_b 40,000; catalogue 128; decision_labels 2 | activity_pages 9,920; all_entities 40,000; fulfillment 80,000; decision_value_join 80,000 |
| 800,000 | ten | 5 | events 639,806; orders 80,000; inventory 64; entity_a 40,000; entity_b 40,000; catalogue 128; decision_labels 2 | activity_pages 9,920; all_entities 40,000; fulfillment 80,000; decision_value_join 80,000 |

## Initialization, load, commit, startup, and open timings

Every timing entry shows `sample count/5; median [min–max]`. Internal example timings are primary. Process wall time includes the runner's 200 ms polling interval and is shown separately; spawn-to-main-entry includes launcher overhead. Startup and world-open are reported independently, never by subtraction.

| Facts | Profile | Phase | Timing metric | Samples; median [min–max] |
|---|---|---|---|---|
| 8,000 | single | setup | init_ms | 5/5; 44.17 ms [38.79–60.55] |
| 8,000 | single | setup | process_wall_ms | 5/5; 212.57 ms [211.52–212.91] |
| 8,000 | single | setup | spawn_to_main_entry_ms | 5/5; 35.88 ms [33.82–37.74] |
| 8,000 | single | load | commit_ms | 5/5; 12,811.04 ms [12,710.77–13,031.09] |
| 8,000 | single | load | fixture_generation_ms | 5/5; 11.59 ms [11.56–11.68] |
| 8,000 | single | load | load_ms | 5/5; 12,822.64 ms [12,722.34–13,042.78] |
| 8,000 | single | load | process_wall_ms | 5/5; 13,081.72 ms [13,034.07–13,283.56] |
| 8,000 | single | load | spawn_to_main_entry_ms | 5/5; 26.89 ms [26.42–27.02] |
| 8,000 | single | load | world_open_ms | 5/5; 31.82 ms [29.21–34.84] |
| 8,000 | single | open-before-edits | process_wall_ms | 5/5; 212.94 ms [205.73–213.98] |
| 8,000 | single | open-before-edits | spawn_to_main_entry_ms | 5/5; 34.84 ms [33.58–35.68] |
| 8,000 | single | open-before-edits | world_open_ms | 5/5; 6.53 ms [6.50–6.62] |
| 8,000 | single | query-before-edits | complete_query_ms | 5/5; 8.99 ms [8.88–9.02] |
| 8,000 | single | query-before-edits | first_page_ms | 5/5; 4.78 ms [4.69–4.82] |
| 8,000 | single | query-before-edits | process_wall_ms | 5/5; 212.55 ms [212.49–214.26] |
| 8,000 | single | query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 1.94 ms [1.91–1.95] |
| 8,000 | single | query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 1.94 ms [1.91–1.95] |
| 8,000 | single | query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 3.90 ms [3.90–3.91] |
| 8,000 | single | query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 3.89 ms [3.88–3.90] |
| 8,000 | single | query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 6.03 ms [5.97–6.27] |
| 8,000 | single | query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 6.00 ms [5.94–6.23] |
| 8,000 | single | query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 6.71 ms [6.68–6.74] |
| 8,000 | single | query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 6.67 ms [6.65–6.71] |
| 8,000 | single | query-before-edits | spawn_to_main_entry_ms | 5/5; 28.24 ms [28.09–28.72] |
| 8,000 | single | query-before-edits | world_open_ms | 5/5; 6.52 ms [6.46–6.67] |
| 8,000 | single | warm-query-before-edits | complete_query_ms | 5/5; 8.33 ms [8.31–8.44] |
| 8,000 | single | warm-query-before-edits | first_page_ms | 5/5; 4.20 ms [4.19–4.30] |
| 8,000 | single | warm-query-before-edits | process_wall_ms | 5/5; 211.94 ms [202.84–211.98] |
| 8,000 | single | warm-query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 1.91 ms [1.90–1.96] |
| 8,000 | single | warm-query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 1.91 ms [1.90–1.96] |
| 8,000 | single | warm-query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 3.61 ms [3.59–3.63] |
| 8,000 | single | warm-query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 3.60 ms [3.57–3.61] |
| 8,000 | single | warm-query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 5.52 ms [5.46–5.69] |
| 8,000 | single | warm-query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 5.49 ms [5.43–5.65] |
| 8,000 | single | warm-query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 6.13 ms [6.07–6.37] |
| 8,000 | single | warm-query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 6.10 ms [6.03–6.33] |
| 8,000 | single | warm-query-before-edits | spawn_to_main_entry_ms | 5/5; 28.12 ms [26.92–28.28] |
| 8,000 | single | warm-query-before-edits | world_open_ms | 5/5; 6.50 ms [6.41–6.52] |
| 8,000 | single | verify-loaded-world | correctness_ms | 5/5; 39,024.26 ms [38,155.13–39,238.34] |
| 8,000 | single | verify-loaded-world | process_wall_ms | 5/5; 39,149.50 ms [38,236.61–39,340.52] |
| 8,000 | single | verify-loaded-world | spawn_to_main_entry_ms | 5/5; 26.59 ms [26.25–26.77] |
| 8,000 | single | edit-add | edit_ms | 5/5; 117.54 ms [112.62–133.53] |
| 8,000 | single | edit-add | process_wall_ms | 5/5; 213.69 ms [211.84–421.61] |
| 8,000 | single | edit-add | spawn_to_main_entry_ms | 5/5; 33.79 ms [32.34–116.51] |
| 8,000 | single | edit-retract | edit_ms | 5/5; 108.97 ms [100.63–124.37] |
| 8,000 | single | edit-retract | process_wall_ms | 5/5; 202.93 ms [201.87–211.50] |
| 8,000 | single | edit-retract | spawn_to_main_entry_ms | 5/5; 26.48 ms [26.19–27.93] |
| 8,000 | single | verify-after-edits | correctness_ms | 5/5; 38,745.12 ms [38,200.16–39,160.34] |
| 8,000 | single | verify-after-edits | process_wall_ms | 5/5; 38,964.12 ms [38,375.14–39,421.80] |
| 8,000 | single | verify-after-edits | spawn_to_main_entry_ms | 5/5; 26.13 ms [25.96–26.19] |
| 8,000 | single | cleanup | process_wall_ms | 5/5; 7.90 ms [6.20–8.83] |
| 8,000 | ten | setup | init_ms | 5/5; 51.22 ms [44.63–55.62] |
| 8,000 | ten | setup | process_wall_ms | 5/5; 211.47 ms [203.87–211.82] |
| 8,000 | ten | setup | spawn_to_main_entry_ms | 5/5; 35.85 ms [34.76–36.57] |
| 8,000 | ten | load | commit_ms | 5/5; 12,708.59 ms [12,520.40–13,086.01] |
| 8,000 | ten | load | fixture_generation_ms | 5/5; 11.63 ms [11.55–11.97] |
| 8,000 | ten | load | load_ms | 5/5; 12,720.32 ms [12,532.05–13,098.01] |
| 8,000 | ten | load | process_wall_ms | 5/5; 12,982.31 ms [12,847.76–13,252.50] |
| 8,000 | ten | load | spawn_to_main_entry_ms | 5/5; 26.94 ms [26.41–27.09] |
| 8,000 | ten | load | world_open_ms | 5/5; 30.75 ms [27.86–41.97] |
| 8,000 | ten | open-before-edits | process_wall_ms | 5/5; 211.46 ms [202.48–213.12] |
| 8,000 | ten | open-before-edits | spawn_to_main_entry_ms | 5/5; 33.15 ms [32.51–35.43] |
| 8,000 | ten | open-before-edits | world_open_ms | 5/5; 2.72 ms [2.70–2.77] |
| 8,000 | ten | query-before-edits | complete_query_ms | 5/5; 7.26 ms [7.22–7.30] |
| 8,000 | ten | query-before-edits | first_page_ms | 5/5; 4.57 ms [4.51–4.58] |
| 8,000 | ten | query-before-edits | process_wall_ms | 5/5; 212.47 ms [212.03–212.69] |
| 8,000 | ten | query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 0.37 ms [0.36–0.38] |
| 8,000 | ten | query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 0.37 ms [0.36–0.38] |
| 8,000 | ten | query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 2.45 ms [2.41–2.48] |
| 8,000 | ten | query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 2.43 ms [2.39–2.46] |
| 8,000 | ten | query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 4.68 ms [4.68–4.80] |
| 8,000 | ten | query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 4.64 ms [4.64–4.76] |
| 8,000 | ten | query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 5.47 ms [5.41–5.49] |
| 8,000 | ten | query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 5.43 ms [5.37–5.45] |
| 8,000 | ten | query-before-edits | spawn_to_main_entry_ms | 5/5; 27.81 ms [26.84–28.57] |
| 8,000 | ten | query-before-edits | world_open_ms | 5/5; 2.72 ms [2.69–2.75] |
| 8,000 | ten | warm-query-before-edits | complete_query_ms | 5/5; 5.37 ms [5.35–5.41] |
| 8,000 | ten | warm-query-before-edits | first_page_ms | 5/5; 2.70 ms [2.68–2.72] |
| 8,000 | ten | warm-query-before-edits | process_wall_ms | 5/5; 212.33 ms [203.54–212.40] |
| 8,000 | ten | warm-query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 0.36 ms [0.34–0.36] |
| 8,000 | ten | warm-query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 0.35 ms [0.33–0.36] |
| 8,000 | ten | warm-query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 2.13 ms [2.10–2.14] |
| 8,000 | ten | warm-query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 2.11 ms [2.08–2.12] |
| 8,000 | ten | warm-query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 4.12 ms [4.10–4.18] |
| 8,000 | ten | warm-query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 4.09 ms [4.07–4.15] |
| 8,000 | ten | warm-query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 4.77 ms [4.72–5.09] |
| 8,000 | ten | warm-query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 4.74 ms [4.68–5.05] |
| 8,000 | ten | warm-query-before-edits | spawn_to_main_entry_ms | 5/5; 27.83 ms [27.33–28.59] |
| 8,000 | ten | warm-query-before-edits | world_open_ms | 5/5; 2.72 ms [2.68–2.74] |
| 8,000 | ten | verify-loaded-world | correctness_ms | 5/5; 38,758.20 ms [37,830.44–39,885.66] |
| 8,000 | ten | verify-loaded-world | process_wall_ms | 5/5; 38,893.04 ms [38,108.14–39,962.84] |
| 8,000 | ten | verify-loaded-world | spawn_to_main_entry_ms | 5/5; 27.29 ms [26.52–27.71] |
| 8,000 | ten | edit-add | edit_ms | 5/5; 109.55 ms [105.49–120.41] |
| 8,000 | ten | edit-add | process_wall_ms | 5/5; 211.44 ms [202.42–213.32] |
| 8,000 | ten | edit-add | spawn_to_main_entry_ms | 5/5; 34.85 ms [32.62–39.75] |
| 8,000 | ten | edit-retract | edit_ms | 5/5; 111.24 ms [107.88–120.25] |
| 8,000 | ten | edit-retract | process_wall_ms | 5/5; 211.37 ms [201.70–211.41] |
| 8,000 | ten | edit-retract | spawn_to_main_entry_ms | 5/5; 26.03 ms [25.97–27.61] |
| 8,000 | ten | verify-after-edits | correctness_ms | 5/5; 38,823.75 ms [38,360.67–39,531.43] |
| 8,000 | ten | verify-after-edits | process_wall_ms | 5/5; 39,037.81 ms [38,556.91–39,757.76] |
| 8,000 | ten | verify-after-edits | spawn_to_main_entry_ms | 5/5; 26.25 ms [26.00–26.52] |
| 8,000 | ten | cleanup | process_wall_ms | 5/5; 14.77 ms [14.50–20.89] |
| 80,000 | single | setup | init_ms | 5/5; 36.91 ms [34.54–41.76] |
| 80,000 | single | setup | process_wall_ms | 5/5; 204.80 ms [201.75–212.50] |
| 80,000 | single | setup | spawn_to_main_entry_ms | 5/5; 33.69 ms [27.67–53.99] |
| 80,000 | single | load | commit_ms | 5/5; 16,541.33 ms [15,533.69–16,755.10] |
| 80,000 | single | load | fixture_generation_ms | 5/5; 116.07 ms [115.01–116.88] |
| 80,000 | single | load | load_ms | 5/5; 16,658.22 ms [15,649.77–16,871.56] |
| 80,000 | single | load | process_wall_ms | 5/5; 16,967.41 ms [16,136.82–17,182.66] |
| 80,000 | single | load | spawn_to_main_entry_ms | 5/5; 31.24 ms [30.15–31.44] |
| 80,000 | single | load | world_open_ms | 5/5; 30.09 ms [28.07–31.41] |
| 80,000 | single | open-before-edits | process_wall_ms | 5/5; 211.43 ms [202.20–214.22] |
| 80,000 | single | open-before-edits | spawn_to_main_entry_ms | 5/5; 26.89 ms [26.25–36.08] |
| 80,000 | single | open-before-edits | world_open_ms | 5/5; 50.83 ms [50.68–57.34] |
| 80,000 | single | query-before-edits | complete_query_ms | 5/5; 43.81 ms [43.58–43.83] |
| 80,000 | single | query-before-edits | first_page_ms | 5/5; 24.03 ms [23.90–24.05] |
| 80,000 | single | query-before-edits | process_wall_ms | 5/5; 1,454.73 ms [1,454.15–1,472.02] |
| 80,000 | single | query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 24.47 ms [24.42–24.63] |
| 80,000 | single | query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 24.38 ms [24.33–24.53] |
| 80,000 | single | query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 154.38 ms [154.30–154.62] |
| 80,000 | single | query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 39.11 ms [39.03–39.18] |
| 80,000 | single | query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 493.07 ms [491.31–494.34] |
| 80,000 | single | query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 62.35 ms [62.12–62.71] |
| 80,000 | single | query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 545.56 ms [544.07–545.81] |
| 80,000 | single | query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 68.47 ms [68.33–68.63] |
| 80,000 | single | query-before-edits | spawn_to_main_entry_ms | 5/5; 30.83 ms [30.45–31.30] |
| 80,000 | single | query-before-edits | world_open_ms | 5/5; 52.31 ms [51.93–53.00] |
| 80,000 | single | warm-query-before-edits | complete_query_ms | 5/5; 39.57 ms [39.26–39.70] |
| 80,000 | single | warm-query-before-edits | first_page_ms | 5/5; 19.89 ms [19.66–20.06] |
| 80,000 | single | warm-query-before-edits | process_wall_ms | 5/5; 2,713.29 ms [2,697.34–2,715.17] |
| 80,000 | single | warm-query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 24.04 ms [23.92–24.14] |
| 80,000 | single | warm-query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 23.95 ms [23.83–24.05] |
| 80,000 | single | warm-query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 154.99 ms [154.15–155.09] |
| 80,000 | single | warm-query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 38.60 ms [38.32–38.86] |
| 80,000 | single | warm-query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 490.29 ms [489.15–490.46] |
| 80,000 | single | warm-query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 61.07 ms [61.02–61.27] |
| 80,000 | single | warm-query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 544.85 ms [542.87–545.72] |
| 80,000 | single | warm-query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 67.80 ms [67.54–69.25] |
| 80,000 | single | warm-query-before-edits | spawn_to_main_entry_ms | 5/5; 28.81 ms [28.14–29.94] |
| 80,000 | single | warm-query-before-edits | world_open_ms | 5/5; 51.30 ms [50.90–51.96] |
| 80,000 | single | verify-loaded-world | correctness_ms | 5/5; 53,076.69 ms [52,145.82–53,556.12] |
| 80,000 | single | verify-loaded-world | process_wall_ms | 5/5; 53,251.51 ms [52,398.59–53,871.15] |
| 80,000 | single | verify-loaded-world | spawn_to_main_entry_ms | 5/5; 28.95 ms [27.70–29.44] |
| 80,000 | single | edit-add | edit_ms | 5/5; 89.36 ms [86.13–90.61] |
| 80,000 | single | edit-add | process_wall_ms | 5/5; 211.80 ms [211.42–415.68] |
| 80,000 | single | edit-add | spawn_to_main_entry_ms | 5/5; 28.32 ms [27.18–34.53] |
| 80,000 | single | edit-retract | edit_ms | 5/5; 94.28 ms [89.08–97.05] |
| 80,000 | single | edit-retract | process_wall_ms | 5/5; 211.40 ms [201.99–214.17] |
| 80,000 | single | edit-retract | spawn_to_main_entry_ms | 5/5; 25.06 ms [25.01–35.98] |
| 80,000 | single | verify-after-edits | correctness_ms | 5/5; 52,087.70 ms [51,258.62–54,688.55] |
| 80,000 | single | verify-after-edits | process_wall_ms | 5/5; 52,182.84 ms [51,384.10–54,921.78] |
| 80,000 | single | verify-after-edits | spawn_to_main_entry_ms | 5/5; 27.88 ms [27.16–28.95] |
| 80,000 | single | cleanup | process_wall_ms | 5/5; 7.68 ms [3.95–8.87] |
| 80,000 | ten | setup | init_ms | 5/5; 51.69 ms [39.24–80.63] |
| 80,000 | ten | setup | process_wall_ms | 5/5; 212.61 ms [203.26–212.74] |
| 80,000 | ten | setup | spawn_to_main_entry_ms | 5/5; 35.20 ms [25.44–40.10] |
| 80,000 | ten | load | commit_ms | 5/5; 18,356.40 ms [17,216.24–18,969.63] |
| 80,000 | ten | load | fixture_generation_ms | 5/5; 118.41 ms [116.10–127.52] |
| 80,000 | ten | load | load_ms | 5/5; 18,472.54 ms [17,343.79–19,088.08] |
| 80,000 | ten | load | process_wall_ms | 5/5; 18,790.34 ms [17,765.67–19,439.20] |
| 80,000 | ten | load | spawn_to_main_entry_ms | 5/5; 26.76 ms [26.31–32.05] |
| 80,000 | ten | load | world_open_ms | 5/5; 30.83 ms [28.52–35.80] |
| 80,000 | ten | open-before-edits | process_wall_ms | 5/5; 211.54 ms [203.11–212.93] |
| 80,000 | ten | open-before-edits | spawn_to_main_entry_ms | 5/5; 33.77 ms [31.49–35.61] |
| 80,000 | ten | open-before-edits | world_open_ms | 5/5; 7.03 ms [6.74–8.00] |
| 80,000 | ten | query-before-edits | complete_query_ms | 5/5; 15.41 ms [14.95–15.65] |
| 80,000 | ten | query-before-edits | first_page_ms | 5/5; 11.00 ms [10.59–11.23] |
| 80,000 | ten | query-before-edits | process_wall_ms | 5/5; 1,235.17 ms [1,033.82–1,262.79] |
| 80,000 | ten | query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 9.42 ms [9.37–9.65] |
| 80,000 | ten | query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 9.32 ms [9.28–9.56] |
| 80,000 | ten | query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 95.87 ms [90.95–98.63] |
| 80,000 | ten | query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 24.82 ms [23.74–25.44] |
| 80,000 | ten | query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 395.47 ms [376.42–414.21] |
| 80,000 | ten | query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 50.45 ms [47.99–54.20] |
| 80,000 | ten | query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 451.73 ms [428.00–470.67] |
| 80,000 | ten | query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 57.27 ms [54.21–58.35] |
| 80,000 | ten | query-before-edits | spawn_to_main_entry_ms | 5/5; 28.66 ms [26.73–33.36] |
| 80,000 | ten | query-before-edits | world_open_ms | 5/5; 7.03 ms [6.80–7.82] |
| 80,000 | ten | warm-query-before-edits | complete_query_ms | 5/5; 8.86 ms [8.42–9.05] |
| 80,000 | ten | warm-query-before-edits | first_page_ms | 5/5; 4.58 ms [4.36–4.75] |
| 80,000 | ten | warm-query-before-edits | process_wall_ms | 5/5; 2,086.61 ms [2,077.97–2,090.25] |
| 80,000 | ten | warm-query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 8.78 ms [8.41–9.17] |
| 80,000 | ten | warm-query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 8.69 ms [8.32–9.07] |
| 80,000 | ten | warm-query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 95.72 ms [90.30–100.68] |
| 80,000 | ten | warm-query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 24.07 ms [22.86–24.82] |
| 80,000 | ten | warm-query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 396.96 ms [374.67–401.76] |
| 80,000 | ten | warm-query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 49.87 ms [46.70–50.09] |
| 80,000 | ten | warm-query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 452.25 ms [426.38–465.69] |
| 80,000 | ten | warm-query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 55.90 ms [53.01–57.41] |
| 80,000 | ten | warm-query-before-edits | spawn_to_main_entry_ms | 5/5; 28.55 ms [26.89–30.61] |
| 80,000 | ten | warm-query-before-edits | world_open_ms | 5/5; 6.80 ms [6.52–6.89] |
| 80,000 | ten | verify-loaded-world | correctness_ms | 5/5; 55,788.91 ms [52,793.68–56,681.63] |
| 80,000 | ten | verify-loaded-world | process_wall_ms | 5/5; 55,944.20 ms [53,043.04–56,836.66] |
| 80,000 | ten | verify-loaded-world | spawn_to_main_entry_ms | 5/5; 26.98 ms [26.10–34.99] |
| 80,000 | ten | edit-add | edit_ms | 5/5; 123.19 ms [86.80–198.78] |
| 80,000 | ten | edit-add | process_wall_ms | 5/5; 406.46 ms [212.70–423.79] |
| 80,000 | ten | edit-add | spawn_to_main_entry_ms | 5/5; 37.20 ms [34.67–44.08] |
| 80,000 | ten | edit-retract | edit_ms | 5/5; 139.39 ms [91.61–161.84] |
| 80,000 | ten | edit-retract | process_wall_ms | 5/5; 211.45 ms [209.90–414.80] |
| 80,000 | ten | edit-retract | spawn_to_main_entry_ms | 5/5; 27.21 ms [25.97–29.32] |
| 80,000 | ten | verify-after-edits | correctness_ms | 5/5; 56,565.87 ms [52,081.98–59,350.19] |
| 80,000 | ten | verify-after-edits | process_wall_ms | 5/5; 56,814.27 ms [52,169.56–59,516.37] |
| 80,000 | ten | verify-after-edits | spawn_to_main_entry_ms | 5/5; 26.73 ms [25.95–31.01] |
| 80,000 | ten | cleanup | process_wall_ms | 5/5; 36.37 ms [6.77–95.94] |
| 800,000 | single | setup | init_ms | 5/5; 25.32 ms [24.12–29.71] |
| 800,000 | single | setup | process_wall_ms | 5/5; 204.97 ms [202.36–412.92] |
| 800,000 | single | setup | spawn_to_main_entry_ms | 5/5; 43.92 ms [38.06–203.29] |
| 800,000 | single | load | commit_ms | 5/5; 86,263.84 ms [85,425.23–88,087.17] |
| 800,000 | single | load | fixture_generation_ms | 5/5; 1,124.05 ms [1,121.14–1,128.05] |
| 800,000 | single | load | load_ms | 5/5; 87,384.99 ms [86,549.29–89,215.24] |
| 800,000 | single | load | process_wall_ms | 5/5; 88,883.81 ms [88,236.16–90,838.19] |
| 800,000 | single | load | spawn_to_main_entry_ms | 5/5; 28.21 ms [26.01–31.41] |
| 800,000 | single | load | world_open_ms | 5/5; 28.45 ms [21.33–29.50] |
| 800,000 | single | open-before-edits | process_wall_ms | 5/5; 626.66 ms [612.80–825.50] |
| 800,000 | single | open-before-edits | spawn_to_main_entry_ms | 5/5; 40.93 ms [38.77–45.88] |
| 800,000 | single | open-before-edits | world_open_ms | 5/5; 530.63 ms [522.37–547.44] |
| 800,000 | single | query-before-edits | complete_query_ms | 5/5; 437.79 ms [431.66–440.23] |
| 800,000 | single | query-before-edits | first_page_ms | 5/5; 252.81 ms [248.90–254.90] |
| 800,000 | single | query-before-edits | process_wall_ms | 5/5; 145,732.33 ms [141,615.01–145,818.71] |
| 800,000 | single | query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 2,739.52 ms [2,719.34–2,746.15] |
| 800,000 | single | query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 272.02 ms [267.07–280.54] |
| 800,000 | single | query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 17,922.20 ms [17,445.24–17,993.35] |
| 800,000 | single | query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 447.52 ms [443.33–482.15] |
| 800,000 | single | query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 59,003.58 ms [57,438.45–59,262.56] |
| 800,000 | single | query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 742.27 ms [729.56–758.27] |
| 800,000 | single | query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 64,464.17 ms [62,752.88–64,785.68] |
| 800,000 | single | query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 816.08 ms [794.87–824.23] |
| 800,000 | single | query-before-edits | spawn_to_main_entry_ms | 5/5; 25.93 ms [25.47–29.64] |
| 800,000 | single | query-before-edits | world_open_ms | 5/5; 528.03 ms [517.55–531.11] |
| 800,000 | single | warm-query-before-edits | complete_query_ms | 5/5; 373.19 ms [372.18–375.61] |
| 800,000 | single | warm-query-before-edits | first_page_ms | 5/5; 187.20 ms [185.91–188.45] |
| 800,000 | single | warm-query-before-edits | process_wall_ms | 5/5; 291,252.70 ms [289,870.01–291,610.83] |
| 800,000 | single | warm-query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 2,731.61 ms [2,716.62–2,782.54] |
| 800,000 | single | warm-query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 272.38 ms [269.99–276.15] |
| 800,000 | single | warm-query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 17,939.17 ms [17,830.27–18,045.57] |
| 800,000 | single | warm-query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 453.56 ms [439.71–468.09] |
| 800,000 | single | warm-query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 59,374.83 ms [59,096.36–59,589.95] |
| 800,000 | single | warm-query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 747.94 ms [735.83–755.76] |
| 800,000 | single | warm-query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 65,064.27 ms [64,566.71–65,220.37] |
| 800,000 | single | warm-query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 813.64 ms [798.00–829.31] |
| 800,000 | single | warm-query-before-edits | spawn_to_main_entry_ms | 5/5; 31.34 ms [27.27–34.31] |
| 800,000 | single | warm-query-before-edits | world_open_ms | 5/5; 526.21 ms [522.99–549.90] |
| 800,000 | single | verify-loaded-world | correctness_ms | 5/5; 443,968.27 ms [441,583.51–446,458.32] |
| 800,000 | single | verify-loaded-world | process_wall_ms | 5/5; 444,820.41 ms [442,431.57–447,327.77] |
| 800,000 | single | verify-loaded-world | spawn_to_main_entry_ms | 5/5; 32.24 ms [30.82–34.18] |
| 800,000 | single | edit-add | edit_ms | 5/5; 101.10 ms [92.88–112.04] |
| 800,000 | single | edit-add | process_wall_ms | 5/5; 840.67 ms [829.87–843.32] |
| 800,000 | single | edit-add | spawn_to_main_entry_ms | 5/5; 44.65 ms [34.29–45.08] |
| 800,000 | single | edit-retract | edit_ms | 5/5; 94.41 ms [89.75–98.58] |
| 800,000 | single | edit-retract | process_wall_ms | 5/5; 205.87 ms [201.86–211.80] |
| 800,000 | single | edit-retract | spawn_to_main_entry_ms | 5/5; 30.30 ms [28.03–32.61] |
| 800,000 | single | verify-after-edits | correctness_ms | 5/5; 403,315.97 ms [401,674.02–412,202.70] |
| 800,000 | single | verify-after-edits | process_wall_ms | 5/5; 403,701.23 ms [402,002.54–412,614.85] |
| 800,000 | single | verify-after-edits | spawn_to_main_entry_ms | 5/5; 27.79 ms [27.63–28.38] |
| 800,000 | single | cleanup | process_wall_ms | 5/5; 149.37 ms [138.17–166.15] |
| 800,000 | ten | setup | init_ms | 5/5; 30.21 ms [26.13–32.81] |
| 800,000 | ten | setup | process_wall_ms | 5/5; 204.40 ms [202.27–212.19] |
| 800,000 | ten | setup | spawn_to_main_entry_ms | 5/5; 43.84 ms [41.29–44.18] |
| 800,000 | ten | load | commit_ms | 5/5; 69,298.62 ms [67,693.15–70,364.66] |
| 800,000 | ten | load | fixture_generation_ms | 5/5; 1,124.65 ms [1,122.40–1,135.32] |
| 800,000 | ten | load | load_ms | 5/5; 70,423.32 ms [68,817.04–71,496.56] |
| 800,000 | ten | load | process_wall_ms | 5/5; 71,574.10 ms [69,802.21–72,741.94] |
| 800,000 | ten | load | spawn_to_main_entry_ms | 5/5; 31.01 ms [28.16–32.12] |
| 800,000 | ten | load | world_open_ms | 5/5; 29.35 ms [26.32–30.42] |
| 800,000 | ten | open-before-edits | process_wall_ms | 5/5; 211.67 ms [202.20–211.82] |
| 800,000 | ten | open-before-edits | spawn_to_main_entry_ms | 5/5; 30.67 ms [25.51–33.49] |
| 800,000 | ten | open-before-edits | world_open_ms | 5/5; 42.55 ms [41.28–44.38] |
| 800,000 | ten | query-before-edits | complete_query_ms | 5/5; 90.77 ms [89.17–94.10] |
| 800,000 | ten | query-before-edits | first_page_ms | 5/5; 70.39 ms [69.04–72.72] |
| 800,000 | ten | query-before-edits | process_wall_ms | 5/5; 119,000.98 ms [112,961.54–119,684.93] |
| 800,000 | ten | query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 1,084.43 ms [1,066.43–1,131.80] |
| 800,000 | ten | query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 109.34 ms [106.97–119.21] |
| 800,000 | ten | query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 11,826.64 ms [11,318.94–12,137.78] |
| 800,000 | ten | query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 290.98 ms [284.57–312.00] |
| 800,000 | ten | query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 50,031.12 ms [47,488.01–50,335.65] |
| 800,000 | ten | query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 626.23 ms [600.55–645.11] |
| 800,000 | ten | query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 55,646.07 ms [52,835.15–55,966.59] |
| 800,000 | ten | query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 693.48 ms [668.91–732.69] |
| 800,000 | ten | query-before-edits | spawn_to_main_entry_ms | 5/5; 31.31 ms [30.47–32.02] |
| 800,000 | ten | query-before-edits | world_open_ms | 5/5; 43.09 ms [42.58–43.47] |
| 800,000 | ten | warm-query-before-edits | complete_query_ms | 5/5; 40.80 ms [40.01–41.50] |
| 800,000 | ten | warm-query-before-edits | first_page_ms | 5/5; 20.61 ms [20.19–21.13] |
| 800,000 | ten | warm-query-before-edits | process_wall_ms | 5/5; 237,254.92 ms [226,956.08–240,492.73] |
| 800,000 | ten | warm-query-before-edits | relation_root::activity_pages_complete_query_ms | 5/5; 1,106.13 ms [1,068.06–1,155.36] |
| 800,000 | ten | warm-query-before-edits | relation_root::activity_pages_first_page_ms | 5/5; 108.96 ms [106.00–118.84] |
| 800,000 | ten | warm-query-before-edits | relation_root::all_entities_complete_query_ms | 5/5; 11,794.46 ms [11,329.56–12,014.75] |
| 800,000 | ten | warm-query-before-edits | relation_root::all_entities_first_page_ms | 5/5; 291.27 ms [281.52–299.73] |
| 800,000 | ten | warm-query-before-edits | relation_root::decision_value_join_complete_query_ms | 5/5; 50,091.77 ms [47,821.86–50,925.56] |
| 800,000 | ten | warm-query-before-edits | relation_root::decision_value_join_first_page_ms | 5/5; 614.74 ms [601.89–643.89] |
| 800,000 | ten | warm-query-before-edits | relation_root::fulfillment_complete_query_ms | 5/5; 55,471.71 ms [53,054.81–56,136.27] |
| 800,000 | ten | warm-query-before-edits | relation_root::fulfillment_first_page_ms | 5/5; 700.81 ms [669.61–755.41] |
| 800,000 | ten | warm-query-before-edits | spawn_to_main_entry_ms | 5/5; 32.25 ms [25.79–33.81] |
| 800,000 | ten | warm-query-before-edits | world_open_ms | 5/5; 43.23 ms [41.40–47.12] |
| 800,000 | ten | verify-loaded-world | correctness_ms | 5/5; 410,400.77 ms [398,762.18–417,018.43] |
| 800,000 | ten | verify-loaded-world | process_wall_ms | 5/5; 410,751.49 ms [399,241.42–417,398.00] |
| 800,000 | ten | verify-loaded-world | spawn_to_main_entry_ms | 5/5; 27.55 ms [25.97–33.95] |
| 800,000 | ten | edit-add | edit_ms | 5/5; 94.70 ms [92.07–101.13] |
| 800,000 | ten | edit-add | process_wall_ms | 5/5; 422.60 ms [410.32–424.53] |
| 800,000 | ten | edit-add | spawn_to_main_entry_ms | 5/5; 43.71 ms [43.24–49.04] |
| 800,000 | ten | edit-retract | edit_ms | 5/5; 96.46 ms [94.89–103.20] |
| 800,000 | ten | edit-retract | process_wall_ms | 5/5; 203.92 ms [202.92–211.78] |
| 800,000 | ten | edit-retract | spawn_to_main_entry_ms | 5/5; 34.02 ms [30.15–34.78] |
| 800,000 | ten | verify-after-edits | correctness_ms | 5/5; 411,968.72 ms [397,150.97–416,285.55] |
| 800,000 | ten | verify-after-edits | process_wall_ms | 5/5; 412,224.65 ms [397,445.31–416,593.64] |
| 800,000 | ten | verify-after-edits | spawn_to_main_entry_ms | 5/5; 28.49 ms [26.72–29.22] |
| 800,000 | ten | cleanup | process_wall_ms | 5/5; 161.56 ms [128.87–164.83] |

## Peak resident memory

| Facts | Profile | Phase | Samples; median [min–max] |
|---|---|---|---|
| 8,000 | single | load | 5/5; 177,471,488 B [169,148,416–190,103,552] |
| 8,000 | single | open-before-edits | 5/5; 24,084,480 B [24,068,096–24,084,480] |
| 8,000 | single | query-before-edits | 5/5; 24,068,096 B [24,068,096–24,084,480] |
| 8,000 | single | warm-query-before-edits | 5/5; 24,068,096 B [24,068,096–24,084,480] |
| 8,000 | ten | load | 5/5; 237,355,008 B [223,690,752–245,432,320] |
| 8,000 | ten | open-before-edits | 5/5; 24,068,096 B [24,068,096–24,084,480] |
| 8,000 | ten | query-before-edits | 5/5; 24,068,096 B [24,068,096–24,084,480] |
| 8,000 | ten | warm-query-before-edits | 5/5; 24,084,480 B [24,068,096–24,084,480] |
| 80,000 | single | load | 5/5; 1,342,062,592 B [1,326,022,656–1,360,642,048] |
| 80,000 | single | open-before-edits | 5/5; 51,806,208 B [51,806,208–52,002,816] |
| 80,000 | single | query-before-edits | 5/5; 80,166,912 B [68,108,288–91,127,808] |
| 80,000 | single | warm-query-before-edits | 5/5; 102,842,368 B [61,767,680–109,477,888] |
| 80,000 | ten | load | 5/5; 881,754,112 B [842,596,352–920,141,824] |
| 80,000 | ten | open-before-edits | 5/5; 24,068,096 B [24,068,096–24,068,096] |
| 80,000 | ten | query-before-edits | 5/5; 56,999,936 B [36,093,952–79,937,536] |
| 80,000 | ten | warm-query-before-edits | 5/5; 69,124,096 B [46,956,544–71,401,472] |
| 800,000 | single | load | 5/5; 8,517,468,160 B [5,830,377,472–8,757,624,832] |
| 800,000 | single | open-before-edits | 5/5; 487,374,848 B [475,643,904–507,887,616] |
| 800,000 | single | query-before-edits | 5/5; 771,637,248 B [746,536,960–784,875,520] |
| 800,000 | single | warm-query-before-edits | 5/5; 841,940,992 B [816,054,272–871,563,264] |
| 800,000 | ten | load | 5/5; 5,147,983,872 B [5,031,788,544–5,176,524,800] |
| 800,000 | ten | open-before-edits | 5/5; 32,604,160 B [32,342,016–34,127,872] |
| 800,000 | ten | query-before-edits | 5/5; 374,472,704 B [318,963,712–383,729,664] |
| 800,000 | ten | warm-query-before-edits | 5/5; 405,422,080 B [371,851,264–410,370,048] |

## Logical and allocated storage

| Facts | Profile | Component | Measure | Samples; median [min–max] |
|---|---|---|---|---|
| 8,000 | single | objects_source_state_and_indexes | allocated | 5/5; 10,846,208 B [10,846,208–10,846,208] |
| 8,000 | single | objects_source_state_and_indexes | logical | 5/5; 10,837,998 B [10,837,998–10,837,998] |
| 8,000 | single | receipts | allocated | 5/5; 4,096 B [4,096–4,096] |
| 8,000 | single | receipts | logical | 5/5; 235 B [235–235] |
| 8,000 | single | revision_journal_and_decisions | allocated | 5/5; 516,096 B [516,096–516,096] |
| 8,000 | single | revision_journal_and_decisions | logical | 5/5; 507,672 B [507,672–507,672] |
| 8,000 | single | root_metadata | allocated | 5/5; 24,576 B [24,576–24,576] |
| 8,000 | single | root_metadata | logical | 5/5; 7,873 B [7,873–7,873] |
| 8,000 | single | staging_scratch | allocated | 5/5; 0 B [0–0] |
| 8,000 | single | staging_scratch | logical | 5/5; 0 B [0–0] |
| 8,000 | single | TOTAL | logical | 5/5; 11,353,778 B [11,353,778–11,353,778] |
| 8,000 | single | TOTAL | allocated | 5/5; 11,390,976 B [11,390,976–11,390,976] |
| 8,000 | ten | objects_source_state_and_indexes | allocated | 5/5; 16,977,920 B [16,977,920–16,977,920] |
| 8,000 | ten | objects_source_state_and_indexes | logical | 5/5; 16,929,587 B [16,929,587–16,929,587] |
| 8,000 | ten | receipts | allocated | 5/5; 40,960 B [40,960–40,960] |
| 8,000 | ten | receipts | logical | 5/5; 2,351 B [2,351–2,351] |
| 8,000 | ten | revision_journal_and_decisions | allocated | 5/5; 573,440 B [573,440–573,440] |
| 8,000 | ten | revision_journal_and_decisions | logical | 5/5; 525,427 B [525,427–525,427] |
| 8,000 | ten | root_metadata | allocated | 5/5; 24,576 B [24,576–24,576] |
| 8,000 | ten | root_metadata | logical | 5/5; 7,877 B [7,877–7,877] |
| 8,000 | ten | staging_scratch | allocated | 5/5; 0 B [0–0] |
| 8,000 | ten | staging_scratch | logical | 5/5; 0 B [0–0] |
| 8,000 | ten | TOTAL | logical | 5/5; 17,465,242 B [17,465,242–17,465,242] |
| 8,000 | ten | TOTAL | allocated | 5/5; 17,616,896 B [17,616,896–17,616,896] |
| 80,000 | single | objects_source_state_and_indexes | allocated | 5/5; 111,251,456 B [111,251,456–111,251,456] |
| 80,000 | single | objects_source_state_and_indexes | logical | 5/5; 111,244,388 B [111,244,388–111,244,388] |
| 80,000 | single | receipts | allocated | 5/5; 4,096 B [4,096–4,096] |
| 80,000 | single | receipts | logical | 5/5; 235 B [235–235] |
| 80,000 | single | revision_journal_and_decisions | allocated | 5/5; 5,263,360 B [5,263,360–5,263,360] |
| 80,000 | single | revision_journal_and_decisions | logical | 5/5; 5,252,476 B [5,252,476–5,252,476] |
| 80,000 | single | root_metadata | allocated | 5/5; 24,576 B [24,576–24,576] |
| 80,000 | single | root_metadata | logical | 5/5; 7,879 B [7,879–7,879] |
| 80,000 | single | staging_scratch | allocated | 5/5; 0 B [0–0] |
| 80,000 | single | staging_scratch | logical | 5/5; 0 B [0–0] |
| 80,000 | single | TOTAL | logical | 5/5; 116,504,978 B [116,504,978–116,504,978] |
| 80,000 | single | TOTAL | allocated | 5/5; 116,543,488 B [116,543,488–116,543,488] |
| 80,000 | ten | objects_source_state_and_indexes | allocated | 5/5; 127,111,168 B [127,111,168–127,111,168] |
| 80,000 | ten | objects_source_state_and_indexes | logical | 5/5; 127,059,952 B [127,059,952–127,059,952] |
| 80,000 | ten | receipts | allocated | 5/5; 40,960 B [40,960–40,960] |
| 80,000 | ten | receipts | logical | 5/5; 2,351 B [2,351–2,351] |
| 80,000 | ten | revision_journal_and_decisions | allocated | 5/5; 5,324,800 B [5,324,800–5,324,800] |
| 80,000 | ten | revision_journal_and_decisions | logical | 5/5; 5,271,758 B [5,271,758–5,271,758] |
| 80,000 | ten | root_metadata | allocated | 5/5; 24,576 B [24,576–24,576] |
| 80,000 | ten | root_metadata | logical | 5/5; 7,883 B [7,883–7,883] |
| 80,000 | ten | staging_scratch | allocated | 5/5; 0 B [0–0] |
| 80,000 | ten | staging_scratch | logical | 5/5; 0 B [0–0] |
| 80,000 | ten | TOTAL | logical | 5/5; 132,341,944 B [132,341,944–132,341,944] |
| 80,000 | ten | TOTAL | allocated | 5/5; 132,501,504 B [132,501,504–132,501,504] |
| 800,000 | single | objects_source_state_and_indexes | allocated | 5/5; 1,117,200,384 B [1,116,676,096–1,117,663,232] |
| 800,000 | single | objects_source_state_and_indexes | logical | 5/5; 1,111,157,300 B [1,111,157,300–1,111,157,300] |
| 800,000 | single | receipts | allocated | 5/5; 4,096 B [4,096–4,096] |
| 800,000 | single | receipts | logical | 5/5; 235 B [235–235] |
| 800,000 | single | revision_journal_and_decisions | allocated | 5/5; 52,711,424 B [52,711,424–52,711,424] |
| 800,000 | single | revision_journal_and_decisions | logical | 5/5; 52,700,481 B [52,700,481–52,700,481] |
| 800,000 | single | root_metadata | allocated | 5/5; 24,576 B [24,576–24,576] |
| 800,000 | single | root_metadata | logical | 5/5; 7,885 B [7,885–7,885] |
| 800,000 | single | staging_scratch | allocated | 5/5; 0 B [0–0] |
| 800,000 | single | staging_scratch | logical | 5/5; 0 B [0–0] |
| 800,000 | single | TOTAL | logical | 5/5; 1,163,865,901 B [1,163,865,901–1,163,865,901] |
| 800,000 | single | TOTAL | allocated | 5/5; 1,169,940,480 B [1,169,416,192–1,170,403,328] |
| 800,000 | ten | objects_source_state_and_indexes | allocated | 5/5; 1,307,680,768 B [1,301,245,952–1,327,005,696] |
| 800,000 | ten | objects_source_state_and_indexes | logical | 5/5; 1,262,215,015 B [1,262,215,015–1,262,215,015] |
| 800,000 | ten | receipts | allocated | 5/5; 40,960 B [40,960–40,960] |
| 800,000 | ten | receipts | logical | 5/5; 2,351 B [2,351–2,351] |
| 800,000 | ten | revision_journal_and_decisions | allocated | 5/5; 52,764,672 B [52,764,672–52,764,672] |
| 800,000 | ten | revision_journal_and_decisions | logical | 5/5; 52,719,505 B [52,719,505–52,719,505] |
| 800,000 | ten | root_metadata | allocated | 5/5; 24,576 B [24,576–24,576] |
| 800,000 | ten | root_metadata | logical | 5/5; 7,889 B [7,889–7,889] |
| 800,000 | ten | staging_scratch | allocated | 5/5; 0 B [0–0] |
| 800,000 | ten | staging_scratch | logical | 5/5; 0 B [0–0] |
| 800,000 | ten | TOTAL | logical | 5/5; 1,314,944,760 B [1,314,944,760–1,314,944,760] |
| 800,000 | ten | TOTAL | allocated | 5/5; 1,360,510,976 B [1,354,076,160–1,379,835,904] |

The ten-batch logical budget is `16 MiB + 12 KiB × source facts`; allocated bytes are reported separately and do not determine this logical-byte gate. Source tuples, retained trie versions, and operator state share CAS packs. Their shared physical bytes are reported together in `objects_source_state_and_indexes`; source tuple/key payload counts are non-additive logical content, not a partition of those physical bytes.

| Facts | Ten-batch logical limit | Measured logical bytes; median [min–max] | Gate |
|---|---|---|---|
| 8,000 | 115,081,216 B (109.75 MiB) | 5/5; 17,465,242 B [17,465,242–17,465,242] | PASS |
| 80,000 | 999,817,216 B (953.50 MiB) | 5/5; 132,341,944 B [132,341,944–132,341,944] | PASS |
| 800,000 | 9,847,177,216 B (9,391.00 MiB) | 5/5; 1,314,944,760 B [1,314,944,760–1,314,944,760] | PASS |

## Named-query pagination

Fresh and warm query processes are separate. Named-query byte counts are rendered-row bytes. First-page row/byte values are read from the raw capture when it remains accessible; all timing, full-result, work, match, and expression statistics come from the validated summary.

| Facts | Profile | Pass | First-page time | Full-query time | First rows | First bytes | Full rows | Full bytes | Work units | Matches | Expressions |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 8,000 | single | fresh | 5/5; 4.78 ms [4.69–4.82] | 5/5; 8.99 ms [8.88–9.02] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 8,000 | single | warm | 5/5; 4.20 ms [4.19–4.30] | 5/5; 8.33 ms [8.31–8.44] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 8,000 | ten | fresh | 5/5; 4.57 ms [4.51–4.58] | 5/5; 7.26 ms [7.22–7.30] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 8,000 | ten | warm | 5/5; 2.70 ms [2.68–2.72] | 5/5; 5.37 ms [5.35–5.41] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 80,000 | single | fresh | 5/5; 24.03 ms [23.90–24.05] | 5/5; 43.81 ms [43.58–43.83] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 80,000 | single | warm | 5/5; 19.89 ms [19.66–20.06] | 5/5; 39.57 ms [39.26–39.70] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 80,000 | ten | fresh | 5/5; 11.00 ms [10.59–11.23] | 5/5; 15.41 ms [14.95–15.65] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 80,000 | ten | warm | 5/5; 4.58 ms [4.36–4.75] | 5/5; 8.86 ms [8.42–9.05] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 800,000 | single | fresh | 5/5; 252.81 ms [248.90–254.90] | 5/5; 437.79 ms [431.66–440.23] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 800,000 | single | warm | 5/5; 187.20 ms [185.91–188.45] | 5/5; 373.19 ms [372.18–375.61] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 800,000 | ten | fresh | 5/5; 70.39 ms [69.04–72.72] | 5/5; 90.77 ms [89.17–94.10] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |
| 800,000 | ten | warm | 5/5; 20.61 ms [20.19–21.13] | 5/5; 40.80 ms [40.01–41.50] | 5/5; 32 [32–32] | 5/5; 1,952 B [1,952–1,952] | 64 | 3,904 | 5/5; 1,024 [1,024–1,024] | 5/5; 0 [0–0] | 5/5; 512 [512–512] |

## Four derived-relation page queries

Derived-relation byte counts are canonical tuple bytes, while named-query byte counts above are rendered-row bytes. These payload measures have different encodings and should not be compared as if they were the same unit. Each timing includes its sample count and median [min–max].

| Facts | Profile | Pass | Relation | First-page time | First rows | First tuple bytes | Full-query time | Full rows | Full tuple bytes |
|---|---|---|---|---|---|---|---|---|---|
| 8,000 | single | fresh | activity_pages | 5/5; 1.94 ms [1.91–1.95] | 20 | 5,132 | 5/5; 1.94 ms [1.91–1.95] | 20 | 5,132 |
| 8,000 | single | fresh | all_entities | 5/5; 3.89 ms [3.88–3.90] | 400 | 37,600 | 5/5; 3.90 ms [3.90–3.91] | 400 | 37,600 |
| 8,000 | single | fresh | fulfillment | 5/5; 6.67 ms [6.65–6.71] | 800 | 82,448 | 5/5; 6.71 ms [6.68–6.74] | 800 | 82,448 |
| 8,000 | single | fresh | decision_value_join | 5/5; 6.00 ms [5.94–6.23] | 800 | 58,256 | 5/5; 6.03 ms [5.97–6.27] | 800 | 58,256 |
| 8,000 | single | warm | activity_pages | 5/5; 1.91 ms [1.90–1.96] | 20 | 5,132 | 5/5; 1.91 ms [1.90–1.96] | 20 | 5,132 |
| 8,000 | single | warm | all_entities | 5/5; 3.60 ms [3.57–3.61] | 400 | 37,600 | 5/5; 3.61 ms [3.59–3.63] | 400 | 37,600 |
| 8,000 | single | warm | fulfillment | 5/5; 6.10 ms [6.03–6.33] | 800 | 82,448 | 5/5; 6.13 ms [6.07–6.37] | 800 | 82,448 |
| 8,000 | single | warm | decision_value_join | 5/5; 5.49 ms [5.43–5.65] | 800 | 58,256 | 5/5; 5.52 ms [5.46–5.69] | 800 | 58,256 |
| 8,000 | ten | fresh | activity_pages | 5/5; 0.37 ms [0.36–0.38] | 20 | 5,132 | 5/5; 0.37 ms [0.36–0.38] | 20 | 5,132 |
| 8,000 | ten | fresh | all_entities | 5/5; 2.43 ms [2.39–2.46] | 400 | 37,600 | 5/5; 2.45 ms [2.41–2.48] | 400 | 37,600 |
| 8,000 | ten | fresh | fulfillment | 5/5; 5.43 ms [5.37–5.45] | 800 | 82,448 | 5/5; 5.47 ms [5.41–5.49] | 800 | 82,448 |
| 8,000 | ten | fresh | decision_value_join | 5/5; 4.64 ms [4.64–4.76] | 800 | 58,256 | 5/5; 4.68 ms [4.68–4.80] | 800 | 58,256 |
| 8,000 | ten | warm | activity_pages | 5/5; 0.35 ms [0.33–0.36] | 20 | 5,132 | 5/5; 0.36 ms [0.34–0.36] | 20 | 5,132 |
| 8,000 | ten | warm | all_entities | 5/5; 2.11 ms [2.08–2.12] | 400 | 37,600 | 5/5; 2.13 ms [2.10–2.14] | 400 | 37,600 |
| 8,000 | ten | warm | fulfillment | 5/5; 4.74 ms [4.68–5.05] | 800 | 82,448 | 5/5; 4.77 ms [4.72–5.09] | 800 | 82,448 |
| 8,000 | ten | warm | decision_value_join | 5/5; 4.09 ms [4.07–4.15] | 800 | 58,256 | 5/5; 4.12 ms [4.10–4.18] | 800 | 58,256 |
| 80,000 | single | fresh | activity_pages | 5/5; 24.38 ms [24.33–24.53] | 920 | 236,432 | 5/5; 24.47 ms [24.42–24.63] | 920 | 236,432 |
| 80,000 | single | fresh | all_entities | 5/5; 39.11 ms [39.03–39.18] | 1,024 | 96,256 | 5/5; 154.38 ms [154.30–154.62] | 4,000 | 376,000 |
| 80,000 | single | fresh | fulfillment | 5/5; 68.47 ms [68.33–68.63] | 1,024 | 105,533 | 5/5; 545.56 ms [544.07–545.81] | 8,000 | 824,471 |
| 80,000 | single | fresh | decision_value_join | 5/5; 62.35 ms [62.12–62.71] | 1,024 | 73,339 | 5/5; 493.07 ms [491.31–494.34] | 8,000 | 582,587 |
| 80,000 | single | warm | activity_pages | 5/5; 23.95 ms [23.83–24.05] | 920 | 236,432 | 5/5; 24.04 ms [23.92–24.14] | 920 | 236,432 |
| 80,000 | single | warm | all_entities | 5/5; 38.60 ms [38.32–38.86] | 1,024 | 96,256 | 5/5; 154.99 ms [154.15–155.09] | 4,000 | 376,000 |
| 80,000 | single | warm | fulfillment | 5/5; 67.80 ms [67.54–69.25] | 1,024 | 105,533 | 5/5; 544.85 ms [542.87–545.72] | 8,000 | 824,471 |
| 80,000 | single | warm | decision_value_join | 5/5; 61.07 ms [61.02–61.27] | 1,024 | 73,339 | 5/5; 490.29 ms [489.15–490.46] | 8,000 | 582,587 |
| 80,000 | ten | fresh | activity_pages | 5/5; 9.32 ms [9.28–9.56] | 920 | 236,432 | 5/5; 9.42 ms [9.37–9.65] | 920 | 236,432 |
| 80,000 | ten | fresh | all_entities | 5/5; 24.82 ms [23.74–25.44] | 1,024 | 96,256 | 5/5; 95.87 ms [90.95–98.63] | 4,000 | 376,000 |
| 80,000 | ten | fresh | fulfillment | 5/5; 57.27 ms [54.21–58.35] | 1,024 | 105,533 | 5/5; 451.73 ms [428.00–470.67] | 8,000 | 824,471 |
| 80,000 | ten | fresh | decision_value_join | 5/5; 50.45 ms [47.99–54.20] | 1,024 | 73,339 | 5/5; 395.47 ms [376.42–414.21] | 8,000 | 582,587 |
| 80,000 | ten | warm | activity_pages | 5/5; 8.69 ms [8.32–9.07] | 920 | 236,432 | 5/5; 8.78 ms [8.41–9.17] | 920 | 236,432 |
| 80,000 | ten | warm | all_entities | 5/5; 24.07 ms [22.86–24.82] | 1,024 | 96,256 | 5/5; 95.72 ms [90.30–100.68] | 4,000 | 376,000 |
| 80,000 | ten | warm | fulfillment | 5/5; 55.90 ms [53.01–57.41] | 1,024 | 105,533 | 5/5; 452.25 ms [426.38–465.69] | 8,000 | 824,471 |
| 80,000 | ten | warm | decision_value_join | 5/5; 49.87 ms [46.70–50.09] | 1,024 | 73,339 | 5/5; 396.96 ms [374.67–401.76] | 8,000 | 582,587 |
| 800,000 | single | fresh | activity_pages | 5/5; 272.02 ms [267.07–280.54] | 1,024 | 263,160 | 5/5; 2,739.52 ms [2,719.34–2,746.15] | 9,920 | 2,549,432 |
| 800,000 | single | fresh | all_entities | 5/5; 447.52 ms [443.33–482.15] | 1,024 | 96,256 | 5/5; 17,922.20 ms [17,445.24–17,993.35] | 40,000 | 3,760,000 |
| 800,000 | single | fresh | fulfillment | 5/5; 816.08 ms [794.87–824.23] | 1,024 | 105,533 | 5/5; 64,464.17 ms [62,752.88–64,785.68] | 80,000 | 8,244,706 |
| 800,000 | single | fresh | decision_value_join | 5/5; 742.27 ms [729.56–758.27] | 1,024 | 71,680 | 5/5; 59,003.58 ms [57,438.45–59,262.56] | 80,000 | 5,825,882 |
| 800,000 | single | warm | activity_pages | 5/5; 272.38 ms [269.99–276.15] | 1,024 | 263,160 | 5/5; 2,731.61 ms [2,716.62–2,782.54] | 9,920 | 2,549,432 |
| 800,000 | single | warm | all_entities | 5/5; 453.56 ms [439.71–468.09] | 1,024 | 96,256 | 5/5; 17,939.17 ms [17,830.27–18,045.57] | 40,000 | 3,760,000 |
| 800,000 | single | warm | fulfillment | 5/5; 813.64 ms [798.00–829.31] | 1,024 | 105,533 | 5/5; 65,064.27 ms [64,566.71–65,220.37] | 80,000 | 8,244,706 |
| 800,000 | single | warm | decision_value_join | 5/5; 747.94 ms [735.83–755.76] | 1,024 | 71,680 | 5/5; 59,374.83 ms [59,096.36–59,589.95] | 80,000 | 5,825,882 |
| 800,000 | ten | fresh | activity_pages | 5/5; 109.34 ms [106.97–119.21] | 1,024 | 263,160 | 5/5; 1,084.43 ms [1,066.43–1,131.80] | 9,920 | 2,549,432 |
| 800,000 | ten | fresh | all_entities | 5/5; 290.98 ms [284.57–312.00] | 1,024 | 96,256 | 5/5; 11,826.64 ms [11,318.94–12,137.78] | 40,000 | 3,760,000 |
| 800,000 | ten | fresh | fulfillment | 5/5; 693.48 ms [668.91–732.69] | 1,024 | 105,533 | 5/5; 55,646.07 ms [52,835.15–55,966.59] | 80,000 | 8,244,706 |
| 800,000 | ten | fresh | decision_value_join | 5/5; 626.23 ms [600.55–645.11] | 1,024 | 71,680 | 5/5; 50,031.12 ms [47,488.01–50,335.65] | 80,000 | 5,825,882 |
| 800,000 | ten | warm | activity_pages | 5/5; 108.96 ms [106.00–118.84] | 1,024 | 263,160 | 5/5; 1,106.13 ms [1,068.06–1,155.36] | 9,920 | 2,549,432 |
| 800,000 | ten | warm | all_entities | 5/5; 291.27 ms [281.52–299.73] | 1,024 | 96,256 | 5/5; 11,794.46 ms [11,329.56–12,014.75] | 40,000 | 3,760,000 |
| 800,000 | ten | warm | fulfillment | 5/5; 700.81 ms [669.61–755.41] | 1,024 | 105,533 | 5/5; 55,471.71 ms [53,054.81–56,136.27] | 80,000 | 8,244,706 |
| 800,000 | ten | warm | decision_value_join | 5/5; 614.74 ms [601.89–643.89] | 1,024 | 71,680 | 5/5; 50,091.77 ms [47,821.86–50,925.56] | 80,000 | 5,825,882 |

## One-row edits and commit counters

These are the benchmark's measured add-one-row and retract-one-row operations. Counters are emitted by the real commit/operator instrumentation; no counts are inferred from elapsed time. Every numeric column shows the sample count and median [min–max].

| Facts | Profile | Edit | Commit time | Operator work | Matches | Expressions evaluated | State node reads | State bytes read | State bytes written | Objects written | Chunks |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 8,000 | single | add one row | 5/5; 117.54 ms [112.62–133.53] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 11 [11–11] | 5/5; 1,198,191 [1,198,191–1,198,191] | 5/5; 1,204,800 [1,204,800–1,204,800] | 5/5; 18 [18–18] | 5/5; 1 [1–1] |
| 8,000 | single | retract one row | 5/5; 108.97 ms [100.63–124.37] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 14 [14–14] | 5/5; 1,198,920 [1,198,920–1,198,920] | 5/5; 1,980 [1,980–1,980] | 5/5; 4 [4–4] | 5/5; 1 [1–1] |
| 8,000 | ten | add one row | 5/5; 109.55 ms [105.49–120.41] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 12 [12–12] | 5/5; 1,198,361 [1,198,361–1,198,361] | 5/5; 1,205,206 [1,205,206–1,205,206] | 5/5; 19 [19–19] | 5/5; 1 [1–1] |
| 8,000 | ten | retract one row | 5/5; 111.24 ms [107.88–120.25] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 14 [14–14] | 5/5; 1,198,920 [1,198,920–1,198,920] | 5/5; 2,150 [2,150–2,150] | 5/5; 4 [4–4] | 5/5; 1 [1–1] |
| 80,000 | single | add one row | 5/5; 89.36 ms [86.13–90.61] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 15 [15–15] | 5/5; 1,200,052 [1,200,052–1,200,052] | 5/5; 1,207,379 [1,207,379–1,207,379] | 5/5; 22 [22–22] | 5/5; 1 [1–1] |
| 80,000 | single | retract one row | 5/5; 94.28 ms [89.08–97.05] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 19 [19–19] | 5/5; 1,200,822 [1,200,822–1,200,822] | 5/5; 1,980 [1,980–1,980] | 5/5; 4 [4–4] | 5/5; 1 [1–1] |
| 80,000 | ten | add one row | 5/5; 123.19 ms [86.80–198.78] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 16 [16–16] | 5/5; 1,200,222 [1,200,222–1,200,222] | 5/5; 1,207,785 [1,207,785–1,207,785] | 5/5; 23 [23–23] | 5/5; 1 [1–1] |
| 80,000 | ten | retract one row | 5/5; 139.39 ms [91.61–161.84] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 19 [19–19] | 5/5; 1,200,822 [1,200,822–1,200,822] | 5/5; 2,150 [2,150–2,150] | 5/5; 4 [4–4] | 5/5; 1 [1–1] |
| 800,000 | single | add one row | 5/5; 101.10 ms [92.88–112.04] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 18 [18–18] | 5/5; 1,201,342 [1,201,342–1,201,342] | 5/5; 1,209,284 [1,209,284–1,209,284] | 5/5; 25 [25–25] | 5/5; 1 [1–1] |
| 800,000 | single | retract one row | 5/5; 94.41 ms [89.75–98.58] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 21 [21–21] | 5/5; 1,202,071 [1,202,071–1,202,071] | 5/5; 1,980 [1,980–1,980] | 5/5; 4 [4–4] | 5/5; 1 [1–1] |
| 800,000 | ten | add one row | 5/5; 94.70 ms [92.07–101.13] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 19 [19–19] | 5/5; 1,201,512 [1,201,512–1,201,512] | 5/5; 1,209,690 [1,209,690–1,209,690] | 5/5; 26 [26–26] | 5/5; 1 [1–1] |
| 800,000 | ten | retract one row | 5/5; 96.46 ms [94.89–103.20] | 5/5; 3 [3–3] | 5/5; 0 [0–0] | 5/5; 3 [3–3] | 5/5; 21 [21–21] | 5/5; 1,202,071 [1,202,071–1,202,071] | 5/5; 2,150 [2,150–2,150] | 5/5; 4 [4–4] | 5/5; 1 [1–1] |

## Capture audit

Capture timestamps span 2026-11-10 08:56:11–20:18:31 UTC; summed process-monotonic elapsed time is 15,333.49 s. The only discontinuity was 25,605.86 s between the last 800,000-fact ten-commit repetition 5 verify-after-edits phase and the next 80,000-fact setup. It falls around untimed correctness/transition work; the cause was not recorded. Timings use Rust `Instant` and Python monotonic clocks. No measured load, open, or query interval contains the discontinuity.

Power and system load were uncontrolled, the OS cache state is unknown, and process-wall polling adds 200 ms. The old alpha.8 exploratory JSONL records and interrupted partial run were lost during temporary-file cleanup. They are unqualified and are not substituted for or used to infer this fresh alpha.9 qualification.

## Reproduction

From source commit `12e37f8b40f2137516e6c9ede60c7431d942fdd5`, build the release example with that commit embedded, then capture all sizes and profiles with five repetitions. Use a durable scratch root:

```sh
BRIX_BENCH_BUILD_COMMIT=12e37f8b40f2137516e6c9ede60c7431d942fdd5 \
  CARGO_TARGET_DIR=target/alpha9-bench-build \
  cargo build --release -p brix-kb --example alpha9_scale_bench
python3 scripts/alpha9_scale_capture.py \
  target/alpha9-bench-build/release/examples/alpha9_scale_bench \
  docs/performance/raw/alpha9-scale-reproduction.jsonl \
  --sizes 8000 80000 800000 --profiles single ten --repetitions 5 \
  --host-metadata docs/performance/alpha9-host.json --allow-large \
  --scratch-root target/alpha9-benchmark-worlds
python3 scripts/alpha9_scale_report.py \
  docs/performance/raw/alpha9-scale-reproduction.jsonl \
  --output docs/performance/raw/alpha9-scale-reproduction-summary.json
```

## Methodology and limits

The operating-system cache state is unknown; a fresh process does not mean a cleared OS cache. The default node cache is 512 MiB and hydrated chunk persistence is disabled. The runner does not alter these settings. The original alpha.8 exploratory JSONL records and interrupted partial run were lost during temporary-file cleanup; they were unqualified and are not substituted for this alpha.9 matrix.

### Workload and claim boundary


The benchmark example generates a deterministic, public Atlas-shaped fixture
at 8,000, 80,000 and 800,000 source facts. It includes a 5,000-child hot group,
balanced event groups, overlapping union-distinct inputs, a nested-field join,
decisions and a partition-scoped named query. The exact fact distribution and
digest are recorded in [`alpha9-fixture.md`](alpha9-fixture.md) and in every
measurement record. No actual Atlas source dataset was available, so results
must not be presented as Atlas-model measurements or as a direct performance
comparison with Atlas's reported alpha.8 figures.

Both a one-commit load and a ten-commit load are measured. The proposed storage
target applies only to the ten-commit history, immediately after its load and
before incremental edit commits:

```text
maximum logical world bytes = 16 MiB + 12 KiB × source fact count
```

The ten-commit `child-load` JSON record reports logical and allocated bytes by
storage component at that point. Later edit records append history; they do not
change the captured budget measurement and the runner never compacts or removes
revisions inside a measured world. A budget miss is not a pass unless an
explicit revision is accepted.


### Capture phases


`../../scripts/alpha9_scale_capture.py` runs each repetition sequentially in a unique
scratch directory, with only one benchmark world present at a time:

1. `--full` creates an empty world and records initialization.
2. `--child-load DIR` inserts the fixture in one or ten commits and records
   load time, commit counters, counts and storage before edit history.
3. `--child-open DIR`, `--child-query DIR` and `--child-query-warm DIR` run in
   separate fresh processes against that exact loaded world. Warm-query mode
   performs one untimed pagination pass, then times its measured pass.
4. `--child-verify DIR` checks the loaded store, oracle and derived counts
   before edits.
5. `--child-edit DIR add` and `--child-edit DIR retract` each run in separate
   processes and report their commit timing and available counters.
6. A final `--child-verify DIR` checks the edited world.

The capture emits one JSONL record for each process phase plus cleanup. Each
record retains its command, release binary path and SHA-256, commit/toolchain
metadata from the example, fixture identity, platform, elapsed wall time,
launcher-inclusive Python-spawn-to-Rust-main-entry time, peak RSS, free-space
snapshots, exit state, and the example's JSON. On macOS peak RSS comes from an
isolated Python resource launcher reading the one measured child's
`RUSAGE_CHILDREN.ru_maxrss` value in bytes. This avoids `ps`; in the restricted
benchmark shell `/usr/bin/time -l` exits after a denied `sysctl kern.clockrate`
call. The compatibility parser accepts the platform's `NUMBER maximum resident
set size` output order. If RSS or the Rust main-entry timestamp is absent, the
record is marked incomplete and is not a valid sample.

Child output goes to bounded temporary files rather than undrained pipes, so a
large diagnostic stream cannot block the child. A run is refused if scratch
space is below 3 GiB; if it falls below that floor during a phase, the process
group is stopped and the failure is retained. Timeout, OOM/signal, failed
correctness, missing measurements and cleanup failure are also retained. No
retry replaces a failed sample. The runner only removes its own unique
per-repetition directory after all child processes have ended; cleanup failures
are recorded.

The resource launcher records each phase in a separate process: initialization,
load, open, query, verification and edits are isolated. Open and both query
passes run against the loaded world before the standalone correctness check;
that check runs before edit measurements. Warm query follows the separate fresh
query process. The OS cache is labeled **unknown**; the runner does not claim a
cleared cache. Rust main-entry latency is reported from the parent process
spawn timestamp and includes launcher overhead. The harness does not estimate
startup by subtracting world-open latency.

Query records include work units, matches and expressions evaluated, plus
first-page/full rows and bytes and canonical content digests for all four
derived relations. The report checks these content measurements across fresh
and warm query processes and repetitions. Edit records include commit counters
for operator work and state I/O/objects. These are the counters exposed by the
actual query/commit APIs; no work is inferred from elapsed time.
Machine model and installed memory remain unavailable unless supplied by the
operator. The root benchmark operator has separately obtained host metadata
for this machine (Apple M1 Pro, 10 CPUs, 16 GiB RAM, macOS 15.6.1 build 24G90,
`rustc 1.96.1 (31fca3adb 2026-06-26)`). Capture accepts this as an explicit
JSON file with `--host-metadata`; it records the supplied values and their
digest without invoking restricted system-inspection commands. Omitting the
option leaves model and memory unavailable.

## Qualification source

Validated summary schema: `brix.alpha9.scale-summary@1`. Required repetitions: 5. Expected cells: 6. The [validated summary](raw/alpha9-scale-summary.json) is computed from the [raw capture](raw/alpha9-scale-all.jsonl). The full result tables above retain all per-phase statistics, storage components, query rows/bytes and digests, named-query measurements, and edit counters from that summary.
