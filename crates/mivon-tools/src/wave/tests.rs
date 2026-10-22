//! Unit test mwave — perpindahan Verbatim dari `wave.rs` monolitik lawas.
//! Tiap subkomando diuji lewat fungsi murninya (tanpa proses `runner`).

use std::collections::BTreeMap;

use super::compare::compare_data;
use super::data::parse_vcd;
use super::decode::{ahb_decode, apb_decode, axi4lite_decode};
use super::get::get_data;
use super::stats::stats_data;
use super::util::{bits_val, full_name, sample_at, wildcard_match};

fn vcd(extra: bool, cnt_seq: &[&str]) -> String {
    let mut s = String::new();
    s.push_str("$timescale 1ns $end\n");
    s.push_str("$scope module top $end\n");
    s.push_str("$var wire 1 ! clk $end\n");
    s.push_str("$var wire 4 \" cnt $end\n");
    if extra {
        s.push_str("$var wire 2 # extra $end\n");
    }
    s.push_str("$upscope $end\n");
    s.push_str("$enddefinitions $end\n");
    s.push_str("#0\n0!\nb0000 \"\n");
    for (i, v) in cnt_seq.iter().enumerate() {
        let t = (i + 1) * 5;
        s.push_str(&format!("#{}\n", t));
        if i % 2 == 0 {
            s.push_str("1!\n");
        } else {
            s.push_str("0!\n");
        }
        s.push_str(&format!("b{} \"\n", v));
    }
    s
}

#[test]
fn test_compare_identical_vcds() {
    std::fs::write("/tmp/__cmp_ident_a.vcd", vcd(false, &["0010", "0011"])).unwrap();
    std::fs::write("/tmp/__cmp_ident_b.vcd", vcd(false, &["0010", "0011"])).unwrap();
    let a = parse_vcd("/tmp/__cmp_ident_a.vcd").unwrap();
    let b = parse_vcd("/tmp/__cmp_ident_b.vcd").unwrap();
    let r = compare_data(&a, &b);
    assert_eq!(r.common, 2, "2 sinyal umum (clk + cnt)");
    assert!(r.only_a.is_empty(), "tidak ada sinyal hanya di A");
    assert!(r.only_b.is_empty(), "tidak ada sinyal hanya di B");
    assert!(r.mismatches.is_empty(), "VCD identik → 0 mismatch");
    let _ = std::fs::remove_file("/tmp/__cmp_ident_a.vcd");
    let _ = std::fs::remove_file("/tmp/__cmp_ident_b.vcd");
    let _ = a;
}

#[test]
fn test_compare_detects_mismatch_and_only_b() {
    std::fs::write("/tmp/__cmp_diff_a.vcd", vcd(false, &["0010", "0011"])).unwrap();
    std::fs::write("/tmp/__cmp_diff_b.vcd", vcd(true, &["0101", "0101"])).unwrap();
    let a = parse_vcd("/tmp/__cmp_diff_a.vcd").unwrap();
    let b = parse_vcd("/tmp/__cmp_diff_b.vcd").unwrap();
    let r = compare_data(&a, &b);
    // cnt berbeda mulai t=5: a=b0010 vs b=b0101.
    let cnt = r
        .mismatches
        .iter()
        .find(|(name, _, _, _, _)| name == "top.cnt")
        .expect("cnt harus mismatch");
    assert_eq!(cnt.1, 5, "first mismatch di t=5");
    assert_eq!(cnt.2, "0010", "nilai A");
    assert_eq!(cnt.3, "0101", "nilai B");
    assert!(cnt.4 >= 1, "ada sample berbeda");
    // extra hanya di B.
    assert_eq!(r.only_b, vec!["top.extra"], "extra hanya ada di B");
    assert!(r.only_a.is_empty());
    let _ = std::fs::remove_file("/tmp/__cmp_diff_a.vcd");
    let _ = std::fs::remove_file("/tmp/__cmp_diff_b.vcd");
}

#[test]
fn test_tree_indexes_scopes() {
    // VCD dengan 2 scope: top.clk/cnt + top.sub.extra.
    let src = "$timescale 1ns $end\n\
               $scope module top $end\n\
               $var wire 1 ! clk $end\n\
               $scope module sub $end\n\
               $var wire 2 # extra $end\n\
               $upscope $end\n\
               $upscope $end\n\
               $enddefinitions $end\n\
               #0\n0!\nb00 #\n";
    std::fs::write("/tmp/__tree.vcd", src).unwrap();
    let data = parse_vcd("/tmp/__tree.vcd").unwrap();
    // 2 scope: [top] dan [top, sub]; signal terkelompok benar.
    let mut scopes: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for sig in &data.signals {
        *scopes.entry(sig.scope.clone()).or_insert(0) += 1;
    }
    assert_eq!(scopes.len(), 2, "2 scope: {}", data.signals.len());
    assert_eq!(
        scopes.get(&vec!["top".to_string()]),
        Some(&1),
        "top punya 1 signal"
    );
    assert_eq!(
        scopes.get(&vec!["top".to_string(), "sub".to_string()]),
        Some(&1),
        "top.sub punya 1 signal"
    );
    let _ = std::fs::remove_file("/tmp/__tree.vcd");
}

#[test]
fn test_wildcard_match() {
    assert!(wildcard_match("cnt", "cnt"));
    assert!(wildcard_match("c*", "cnt"));
    assert!(wildcard_match("*nt", "cnt"));
    assert!(wildcard_match("*", "anything"));
    assert!(wildcard_match("c?t", "cnt"));
    assert!(wildcard_match("top.*", "top.cnt"));
    assert!(!wildcard_match("c?t", "count"));
    assert!(!wildcard_match("top.*", "other.cnt"));
    assert!(!wildcard_match("cnt", "clk"));
}

#[test]
fn test_get_at_samples_value() {
    std::fs::write("/tmp/__get_at.vcd", vcd(false, &["0010", "0011", "0100"])).unwrap();
    let data = parse_vcd("/tmp/__get_at.vcd").unwrap();
    // cnt berubah di t=0(0000), 5(0010), 10(0011), 15(0100).
    let r = get_data(&data, &["cnt".to_string()], Some(7), None).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, "top.cnt");
    assert_eq!(r[0].width, 4);
    assert_eq!(
        r[0].values,
        vec![(7, "0010".to_string())],
        "@7 = perubahan terakhir ≤ 7 (t=5)"
    );
    // @0 sebelum perubahan pertama cnt tetap b0000 (perubahan #0 ada).
    let r0 = get_data(&data, &["cnt".to_string()], Some(0), None).unwrap();
    assert_eq!(r0[0].values, vec![(0, "0000".to_string())]);
    // Sinyal tanpa perubahan sebelum t → "x".
    let rx = get_data(&data, &["extra".to_string()], Some(3), None);
    assert!(rx.is_err(), "extra tidak ada di VCD ini → error no-match");
    let _ = std::fs::remove_file("/tmp/__get_at.vcd");
}

#[test]
fn test_get_range_filters_changes() {
    std::fs::write(
        "/tmp/__get_range.vcd",
        vcd(false, &["0010", "0011", "0100", "0101"]),
    )
    .unwrap();
    let data = parse_vcd("/tmp/__get_range.vcd").unwrap();
    // cnt changes: 0, 5, 10, 15, 20 → range [5:15] = 3 perubahan.
    let r = get_data(&data, &["top.cnt".to_string()], None, Some((5, 15))).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(
        r[0].values,
        vec![
            (5, "0010".to_string()),
            (10, "0011".to_string()),
            (15, "0100".to_string())
        ],
        "range [5:15] menyaring perubahan di luar window"
    );
    // Range kosong → 0 nilai (sinyal cocok tapi tak ada perubahan).
    let re = get_data(&data, &["cnt".to_string()], None, Some((100, 200))).unwrap();
    assert!(re[0].values.is_empty());
    let _ = std::fs::remove_file("/tmp/__get_range.vcd");
}

#[test]
fn test_get_wildcard_multiple_and_full_timeline() {
    std::fs::write("/tmp/__get_multi.vcd", vcd(false, &["0010", "0011"])).unwrap();
    let data = parse_vcd("/tmp/__get_multi.vcd").unwrap();
    // Pola "c*" cocok clk + cnt; timeline penuh (tanpa at/range).
    let r = get_data(&data, &["c*".to_string()], None, None).unwrap();
    assert_eq!(r.len(), 2, "c* → clk + cnt");
    let clk = r.iter().find(|e| e.name == "top.clk").unwrap();
    assert_eq!(
        clk.values,
        vec![
            (0, "0".to_string()),
            (5, "1".to_string()),
            (10, "0".to_string())
        ],
        "timeline penuh clk"
    );
    // Nama polos (tanpa scope) juga match.
    let r2 = get_data(&data, &["clk".to_string()], None, None).unwrap();
    assert_eq!(r2.len(), 1);
    assert_eq!(r2[0].name, "top.clk");
    // Tidak ada yang cocok → error.
    assert!(get_data(&data, &["nosuch".to_string()], None, None).is_err());
    let _ = std::fs::remove_file("/tmp/__get_multi.vcd");
}

#[test]
fn test_search_finds_signals_by_pattern() {
    std::fs::write("/tmp/__srch.vcd", vcd(true, &["0010"])).unwrap();
    let data = parse_vcd("/tmp/__srch.vcd").unwrap();
    // Pola `c*` cocok clk + cnt (bukan extra); `*extra*` cocok extra.
    let names: Vec<String> = data
        .signals
        .iter()
        .filter(|s| wildcard_match("c*", &s.name) || wildcard_match("*extra*", &s.name))
        .map(full_name)
        .collect();
    assert!(names.contains(&"top.clk".to_string()));
    assert!(names.contains(&"top.cnt".to_string()));
    assert!(names.contains(&"top.extra".to_string()));
    assert_eq!(names.len(), 3);
    // Pola sempit `cnt` hanya 1.
    let cnt_only: Vec<String> = data
        .signals
        .iter()
        .filter(|s| wildcard_match("cnt", &s.name))
        .map(full_name)
        .collect();
    assert_eq!(cnt_only, vec!["top.cnt".to_string()]);
    let _ = std::fs::remove_file("/tmp/__srch.vcd");
}

#[test]
fn test_stats_counts_toggles_and_activity() {
    // clk: 0@0, 1@5, 0@10 → 2 toggle, first=0, last=10.
    // cnt: 0010@0, 0011@5 (1 toggle), 0101@10 (1 toggle) → 2 toggle.
    std::fs::write("/tmp/__st.vcd", vcd(false, &["0010", "0011"])).unwrap();
    let data = parse_vcd("/tmp/__st.vcd").unwrap();
    let r = stats_data(&data);
    assert_eq!(r.len(), 2, "2 sinyal di-stats");
    let clk = r.iter().find(|s| s.name == "top.clk").expect("clk");
    assert_eq!(clk.width, 1);
    assert_eq!(clk.toggles, 2, "clk 0→1→0 = 2 toggle");
    assert_eq!(clk.changes, 3, "3 perubahan (0,5,10)");
    assert_eq!(clk.first, 0, "perubahan pertama di inisialisasi t=0");
    assert_eq!(clk.last, 10);
    // Aktivitas clk: nilai != baseline 0 selama [5,10) = 5 unit + sampai
    // akhir (max_time=10) nilai 0 = 0 → activity 5.
    assert_eq!(clk.activity, 5, "clk aktif 5 unit waktu");
    assert!(clk.duration >= 10);
    let cnt = r.iter().find(|s| s.name == "top.cnt").expect("cnt");
    assert_eq!(cnt.toggles, 2, "cnt 2 transisi nilai");
    assert_eq!(cnt.first, 0, "cnt inisialisasi di t=0");
    assert_eq!(cnt.last, 10);
    let _ = std::fs::remove_file("/tmp/__st.vcd");
}

#[test]
fn test_stats_stuck_signal_zero_toggle() {
    // Signal tanpa perubahan → toggles=0 (stuck) — aktivitas 0.
    let src = "$timescale 1ns $end\n\
               $scope module top $end\n\
               $var wire 1 ! clk $end\n\
               $var wire 4 \" cnt $end\n\
               $upscope $end\n\
               $enddefinitions $end\n\
               #0\n0!\nb0000 \"\n\
               #10\n1!\n";
    std::fs::write("/tmp/__stuck.vcd", src).unwrap();
    let data = parse_vcd("/tmp/__stuck.vcd").unwrap();
    let r = stats_data(&data);
    let cnt = r.iter().find(|s| s.name == "top.cnt").expect("cnt");
    assert_eq!(cnt.toggles, 0, "cnt tidak pernah berubah → stuck");
    assert_eq!(cnt.changes, 1, "hanya inisialisasi");
    assert_eq!(cnt.activity, 0, "aktivitas 0");
    let _ = std::fs::remove_file("/tmp/__stuck.vcd");
}

/// Helper: bangun file VCD dari definisi $var + body mentah (WAV-16).
/// vars: (id, width, name) — semua di scope `top`.
fn vcd_proto(vars: &[(&str, usize, &str)], body: &str) -> String {
    let mut s = String::from("$timescale 1ns $end\n$scope module top $end\n");
    for (id, w, name) in vars {
        s.push_str(&format!("$var wire {} {} {}  $end\n", w, id, name));
    }
    s.push_str("$upscope $end\n$enddefinitions $end\n");
    s.push_str(body);
    s
}

#[test]
fn test_apb_decode_write_read_and_wait_state() {
    // TX1 WRITE 0x10<=0xAB (pready langsung 1).
    // TX2 READ 0x40 dengan WAIT STATE: penable t=60, pready=0 t=60,
    // pready=1 t=70 → transaksi selesai di t=70, rdata=0x51.
    let src = vcd_proto(
        &[
            ("!", 1, "psel"),
            ("\"", 1, "penable"),
            ("#", 1, "pwrite"),
            ("$", 1, "pready"),
            ("%", 8, "paddr"),
            ("&", 8, "pwdata"),
            ("'", 8, "prdata"),
        ],
        "#0\n0!\n0\"\n0#\n1$\nb00000000 %\nb00000000 &\nb00000000 '\n\
         #10\n1!\n1#\nb00010000 %\nb10101011 &\n\
         #20\n1\"\n\
         #30\n0!\n0\"\n0#\n\
         #50\n1!\n0#\nb01000000 %\nb01010001 '\n\
         #60\n1\"\n0$\n\
         #70\n1$\n\
         #80\n0!\n0\"\n",
    );
    std::fs::write("/tmp/__dec_apb.vcd", src).unwrap();
    let data = parse_vcd("/tmp/__dec_apb.vcd").unwrap();
    let txs = apb_decode(&data).expect("apb decode ok");
    assert_eq!(txs.len(), 2, "2 transaksi APB");

    let w = &txs[0];
    assert_eq!(w.kind, "WRITE");
    assert_eq!(w.start, 10);
    assert_eq!(w.end, 20);
    assert_eq!(w.addr, 0x10);
    assert_eq!(w.data, 0xAB);
    assert_eq!(w.resp, None);

    let r = &txs[1];
    assert_eq!(r.kind, "READ");
    assert_eq!(r.start, 50);
    assert_eq!(r.end, 70, "wait state: selesai saat pready naik t=70");
    assert_eq!(r.addr, 0x40);
    assert_eq!(r.data, 0x51);
    let _ = std::fs::remove_file("/tmp/__dec_apb.vcd");
}

#[test]
fn test_axi4lite_decode_write_and_read_channels() {
    // Write: aw handshake t=10, w handshake t=10, b handshake t=20.
    // Read : ar handshake t=40, r handshake t=50 (rresp=2 → 0b10).
    let src = vcd_proto(
        &[
            ("a", 1, "awvalid"),
            ("b", 1, "awready"),
            ("c", 8, "awaddr"),
            ("d", 1, "wvalid"),
            ("e", 1, "wready"),
            ("f", 8, "wdata"),
            ("g", 1, "bvalid"),
            ("h", 1, "bready"),
            ("i", 2, "bresp"),
            ("j", 1, "arvalid"),
            ("k", 1, "arready"),
            ("l", 8, "araddr"),
            ("m", 1, "rvalid"),
            ("n", 1, "rready"),
            ("o", 8, "rdata"),
            ("p", 2, "rresp"),
        ],
        "#0\n0a\n1b\n0d\n1e\n0g\n1h\nb00 i\n0j\n1k\n1n\nb00 p\n\
         #10\n1a\nb00110000 c\n1d\nb11011110 f\n\
         #20\n0a\n0d\n1g\nb01 i\n\
         #30\n0g\n\
         #40\n1j\nb01010000 l\n\
         #50\n0j\n1m\nb01110111 o\nb10 p\n\
         #60\n0m\n",
    );
    std::fs::write("/tmp/__dec_axi.vcd", src).unwrap();
    let data = parse_vcd("/tmp/__dec_axi.vcd").unwrap();
    let mut txs = axi4lite_decode(&data).expect("axi decode ok");
    assert_eq!(txs.len(), 2, "1 write + 1 read");
    txs.sort_by_key(|t| t.start);

    let w = &txs[0];
    assert_eq!(w.kind, "WRITE");
    assert_eq!(w.start, 10);
    assert_eq!(w.end, 20);
    assert_eq!(w.addr, 0x30);
    assert_eq!(w.data, 0xDE);
    assert_eq!(w.resp, Some(0b01), "bresp OKAY=01 tercatat");

    let r = &txs[1];
    assert_eq!(r.kind, "READ");
    assert_eq!(r.start, 40);
    assert_eq!(r.end, 50);
    assert_eq!(r.addr, 0x50);
    assert_eq!(r.data, 0x77);
    assert_eq!(r.resp, Some(0b10));
    let _ = std::fs::remove_file("/tmp/__dec_axi.vcd");
}

#[test]
fn test_ahb_decode_single_transfers() {
    // WRITE: addr phase NONSEQ t=10, data phase selesai t=20 (hwdata).
    // READ : addr phase NONSEQ t=40, data phase selesai t=50 (hrdata).
    let src = vcd_proto(
        &[
            ("a", 2, "htrans"),
            ("b", 1, "hwrite"),
            ("c", 1, "hready"),
            ("d", 8, "haddr"),
            ("e", 8, "hwdata"),
            ("f", 8, "hrdata"),
        ],
        "#0\nb00 a\n0b\n1c\nb00000000 d\ne\nf\n\
         #10\nb10 a\n1b\nb01100000 d\n\
         #20\nb00 a\nb10011010 e\n\
         #40\nb10 a\n0b\nb01100100 d\nb00111110 f\n\
         #50\nb00 a\n",
    );
    std::fs::write("/tmp/__dec_ahb.vcd", src).unwrap();
    let data = parse_vcd("/tmp/__dec_ahb.vcd").unwrap();
    let txs = ahb_decode(&data).expect("ahb decode ok");
    assert_eq!(txs.len(), 2, "2 transfer AHB");

    let w = &txs[0];
    assert_eq!(w.kind, "WRITE");
    assert_eq!(w.start, 10);
    assert_eq!(w.end, 20);
    assert_eq!(w.addr, 0x60);
    assert_eq!(w.data, 0x9A);

    let r = &txs[1];
    assert_eq!(r.kind, "READ");
    assert_eq!(r.start, 40);
    assert_eq!(r.end, 50);
    assert_eq!(r.addr, 0x64);
    assert_eq!(r.data, 0x3E);
    let _ = std::fs::remove_file("/tmp/__dec_ahb.vcd");
}

#[test]
fn test_decode_missing_signal_errors() {
    // VCD tanpa psel → apb_decode error jelas.
    let src = vcd_proto(
        &[("\"", 1, "penable"), ("#", 1, "pwrite"), ("$", 8, "paddr")],
        "#0\n0\"\n0#\n",
    );
    std::fs::write("/tmp/__dec_miss.vcd", src).unwrap();
    let data = parse_vcd("/tmp/__dec_miss.vcd").unwrap();
    let e = apb_decode(&data).expect_err("psel hilang harus error");
    assert!(
        format!("{}", e).contains("psel"),
        "pesan menyebut sinyal yang hilang"
    );
    let _ = std::fs::remove_file("/tmp/__dec_miss.vcd");
}

#[test]
fn test_sample_at_and_bits_val() {
    let tl = vec![(0u64, "0".to_string()), (10, "1".to_string())];
    assert_eq!(sample_at(&tl, 5), "0", "sebelum perubahan kedua");
    assert_eq!(sample_at(&tl, 10), "1");
    assert_eq!(sample_at(&tl, 99), "1", "nilai bertahan");
    assert_eq!(sample_at(&[], 0), "x", "timeline kosong → x");
    assert_eq!(bits_val("1010"), 0xA);
    assert_eq!(bits_val("00000000"), 0);
    assert_eq!(bits_val("xxxx"), 0, "x dianggap 0");
}

/// Regresi pemecahan file: dispatcher `run()` tetap bekerja meneruskan ke
/// submodul tree (jalur modul `args → tree` terjaga setelah refactor).
#[test]
fn test_run_tree_dispatch() {
    use super::args::{run, WaveArgs};
    std::fs::write("/tmp/__run_tree.vcd", vcd(false, &["0010"])).unwrap();
    let args = WaveArgs::Tree {
        input: "/tmp/__run_tree.vcd".to_string(),
    };
    let res = run(&args);
    assert!(res.is_ok(), "run(Tree) harus ok: {:?}", res.err());
    let _ = std::fs::remove_file("/tmp/__run_tree.vcd");
}
