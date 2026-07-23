# Parity checklist (M2 acceptance)

Every row must pass side-by-side against the original client on a live caster and a real
receiver before M2 is called done. "Delta" rows are deliberate improvements, verified to
behave as documented.

A row is checked `[x]` when automated tests (unit / mock-caster integration) or a
`--selftest` run against a live public caster prove the behavior; the Evidence column
names them. Rows needing a physical receiver, a real serial port, or eyes on the GUI stay
unchecked and are annotated "manual e2e" - they close during the pre-tag hardware pass.

**This checklist is a release-tag blocker.** No `vX.Y.Z` tag is pushed, and README's
"parity with the original client" claim does not stand, until every row is `[x]` with its
Evidence column recording the date, caster, and receiver of the live side-by-side run
that closed it. Commit messages saying "M2 done" do not close rows; evidence does.

| # | Behavior | Status | Evidence |
|---|----------|--------|----------|
| 1 | NTRIP v1 connect to caster, `ICY 200 OK` stream starts | [x] | session_flows `icy_*` suite; worker_mock `stream_with_coalesced_payload_counts_every_byte`; live selftest vs two public casters streamed RTCM |
| 2 | Sourcetable download populates mountpoint dropdown | [x] | worker_mock `sourcetable_fetch_saves_cache_and_posts_parsed_table`; live selftest downloaded a 1109-stream table (NTRIP v2 caster) |
| 3 | Bad credentials -> clear "invalid username/password", no auto-reconnect | [x] | worker_mock `unauthorized_stops_without_reconnect_despite_auto_reconnect`; session_flows `unauthorized_v1_style` / `unauthorized_http11_style_with_headers` |
| 4 | Bad mountpoint -> reported plainly (caster returned sourcetable) | [x] | worker_mock `bad_mountpoint_reports_plainly_and_still_surfaces_table`; session_flows `mountpoint_not_found` |
| 5 | Raw TCP mode streams without any HTTP handshake | [x] | worker_mock `raw_tcp_streams_without_request_or_gga_and_posts_rtcm_stats`; session_flows `raw_tcp_streams_immediately_and_never_asks_for_gga` |
| 6 | Corrections forwarded byte-exact to serial port | [ ] | manual e2e (real receiver); write path unit-tested: serial `forward_block_*` / `drain_corrections_*` |
| 7 | Serial settings honored: port, baud, data bits, stop bits (delta: stop bits actually honored), parity | [ ] | manual e2e (real serial hardware) |
| 8 | NovAtel auto-config sends the 4 commands at the configured rate/format | [ ] | manual e2e (NovAtel receiver); command bytes pinned by serial `novatel_command_sequence_exact` |
| 9 | Receiver NMEA parsed: GGA quality/sats/HDOP/alt/age/station, RMC speed/heading, GSA DOPs (delta: PDOP read correctly) | [x] | gnss nmea goldens: `gga_classic_full_decode`, `rmc_classic_full_decode`, `gsa_dop_fields_not_swapped`, `any_talker_accepted`, checksum-reject suite |
| 10 | Fix-quality names match original (Invalid..WAAS) in status + log transitions | [x] | gnss `quality_names_match_original_vocabulary`; serial worker `quality_transitions_named_like_original` |
| 11 | Manual-location GGA: exact template (quality 4, 10 sats, HDOP 1.0, alt 200 m, age (s%6)+3), first send ~0.3 s, then every 10 s when stream requires NMEA (delta in 0.2.1: an unknown requirement counts as required - the CHC APIS fix - and an unset 0,0 manual position sends nothing) | [x] | gnss gga goldens (`golden_portland_north_west`, `age_cycles_three_through_eight`); session_flows `gga_cadence_first_at_300ms_then_10s_after_sent`, `gga_policy_gating`, `gga_missed_retries_on_the_short_slot`; worker_mock `always_gga_from_manual_position_reaches_the_wire`, `apis_style_caster_streams_only_after_gga` |
| 12 | Receiver-GGA passthrough mode sends the receiver's sentence verbatim | [x] | worker_mock `receiver_gga_passthrough_sends_last_sentence_verbatim`; serial `line_assembly_updates_last_gga_slot` |
| 13 | Delta: no loss of correction bytes that arrive with the `ICY 200 OK` segment | [x] | session_flows `icy_coalesced_payload_regression` (+ split/blank-line variants); worker_mock `stream_with_coalesced_payload_counts_every_byte` |
| 14 | 30 s first-response timeout, 30 s stream-silence timeout | [x] | session_flows `first_response_timeout`, `first_response_timeout_with_partial_headers`, `stream_silence_timeout`, `sourcetable_timeout` |
| 15 | Auto-reconnect every ~10 s with attempt counter; .wav alert on drop | [x] | worker_mock `drop_with_auto_reconnect_waits_and_cancel_cuts_the_wait_short`, `established_stream_drop_alerts_even_without_reconnect`; ntrip worker `reconnect_gating_truth_table` (audible playback itself: manual e2e) |
| 16 | Event log pane + optional `Logs\YYYYMMDD.txt` with parity vocabulary | [x] | logging `sink_rolls_at_date_change_and_logs_it`, `logger_thread_writes_flushes_and_shuts_down`; ntrip worker `mib_milestones`, `close_summaries_are_plain_words` |
| 17 | Optional `NMEA\YYYYMMDD.txt` recording | [x] | same daily-sink test suite (logging.rs) drives the NMEA sink; `disabled_sink_writes_nothing_and_creates_no_dir` covers the off state |
| 18 | Elevation graph: start/pause/reset, min/max/current/range | [ ] | manual e2e (GUI) |
| 19 | Two configurable status readout slots (age/DOPs/elevation/speed/heading ids) | [x] | settings `display_id_parsing_covers_all_legacy_ids`; gnss `unit_conversions` (rendering itself: manual GUI check) |
| 20 | Settings persist next to exe; window geometry restored | [ ] | manual e2e (GUI geometry restore); TOML persistence unit-tested: settings `default_roundtrips_through_toml`, `unknown_keys_ignored_on_load_dropped_on_save` |
| 21 | First run imports `Settings.txt` + `ntripconfig.txt` + `sourcetable.dat`, files left untouched, import logged | [x] | settings `import_happy_path_both_files`, `import_leaves_legacy_files_untouched_and_first_run_saves_toml`, `legacy_parser_rules_exact`, `import_keys_case_insensitive_and_unknown_logged` |
| 22 | Portable: no writes outside the exe folder (no AppData, no registry) | [ ] | manual e2e (Process Monitor audit of a full session; eframe persistence is compile-time off) |
| 23 | Green data-activity indicator pulses per correction burst (delta: adds a live kB/s caption and an orange "no data N s" stall readout) | [x] | status_strip `activity_decay_half_life`, `stream_classification_truth_table`, `cluster_tokens_and_fill`; state `ntrip_rx_age_and_stall_predicate`; release GUI driven against a mock stream 2026-07-16 - screenshots showed idle, pulsing + rate, stalled-orange, and reconnect-wait states; status_strip `advance_activity_decays_then_snaps_on_growth` pins the drain-loop envelope seam; min window width raised to the 760 default (main.rs) after a 700 px launch clipped the cluster and log buttons off-screen |
