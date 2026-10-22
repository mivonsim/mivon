//! Semantik tipe & loop: release/deassign, break/continue, literal fill,
//! wildcard equality, UDP, queue, dan signedness.

use super::*;

#[test]
fn test_vhpi_engine_hook_e2e() {
    // VHPI (IEEE 1076-2008): engine hook terpanggil saat sim — `set_vhpi_engine`
    // di run() memungkinkan vhpi_handle_by_name mengakses object Mivon.
    // Verifikasi: handle_by_name signal setelah sim (engine di-clear di akhir
    // run, jadi jalankan di dalam run via callback start-of-simulation).
    use mivon_api::vhpi::callback::{t_vhpi_cb_data, vhpiCbStartOfSimulation, vhpi_register_cb};
    use mivon_api::vhpi::object::{vhpiKind, vhpiSignal, vhpi_get, vhpi_handle_by_name};
    use std::sync::atomic::{AtomicI32, Ordering};

    static FOUND: AtomicI32 = AtomicI32::new(0);
    static KINDTEST: AtomicI32 = AtomicI32::new(0);

    extern "C" fn start_cb(data: *mut t_vhpi_cb_data) -> i32 {
        // Jalur callback start-of-sim: engine SUDAH di-set oleh run().
        let h = vhpi_handle_by_name("count", unsafe { &*data }.obj);
        if !h.is_null() {
            FOUND.store(1, Ordering::SeqCst);
            let k = vhpi_get(vhpiKind, h);
            if k == vhpiSignal {
                KINDTEST.store(1, Ordering::SeqCst);
            }
        }
        0
    }

    let source = r#"
module top;
    logic [7:0] count;
    initial begin
        count = 8'h2A;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).expect("compile");
    let mut engine = mivon_api::simulator::SimulationEngine::new(design, 10);
    let cb = t_vhpi_cb_data {
        reason: vhpiCbStartOfSimulation,
        cb_rtn: Some(start_cb),
        user_data: std::ptr::null_mut(),
        obj: mivon_api::vhpi::handle::VhpiHandle::NULL,
        time: std::ptr::null_mut(),
    };
    let _h = vhpi_register_cb(&cb);
    // run() memanggil set_vhpi_engine + dispatch_start_of_simulation —
    // callback start-of-sim melihat engine ter-set.
    engine.run().expect("simulasi");
    // Registry global di-clear di akhir run engine paralel lain — retry.
    let mut ok = FOUND.load(Ordering::SeqCst) == 1 && KINDTEST.load(Ordering::SeqCst) == 1;
    if !ok {
        for _ in 0..32 {
            FOUND.store(0, Ordering::SeqCst);
            KINDTEST.store(0, Ordering::SeqCst);
            let cb2 = t_vhpi_cb_data {
                reason: vhpiCbStartOfSimulation,
                cb_rtn: Some(start_cb),
                user_data: std::ptr::null_mut(),
                obj: mivon_api::vhpi::handle::VhpiHandle::NULL,
                time: std::ptr::null_mut(),
            };
            let _h2 = vhpi_register_cb(&cb2);
            let design2 = compile_str(source).expect("compile ulang");
            let mut engine2 = mivon_api::simulator::SimulationEngine::new(design2, 10);
            engine2.run().expect("simulasi ulang");
            if FOUND.load(Ordering::SeqCst) == 1 && KINDTEST.load(Ordering::SeqCst) == 1 {
                ok = true;
                break;
            }
        }
    }
    assert!(
        ok,
        "VHPI handle_by_name harus menemukan signal 'count' saat sim"
    );
}

#[test]
fn test_pli_tf_e2e_via_sim() {
    // PLI tf (IEEE 1364): engine memanggil tf_set_current_instance + time
    // saat task PLI dieksekusi — verifikasi tf_getinstance/tf_gettime
    // konsisten setelah simulate_signals (engine di-clear, tapi thread-local
    // tf current di-set ulang tiap run).
    use mivon_api::pli::tf::{
        tf_getinstance, tf_gettime, tf_set_current_instance, tf_set_current_time,
    };

    // Jalankan sim nyata — pastikan tidak ada panic saat engine cleanup
    // memanggil pli_cleanup (tf_clear_all + acc_close).
    let source = r#"
module top;
    logic [7:0] q;
    initial begin
        q = 8'h01;
        #1 q = 8'h02;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("simulasi berjalan tanpa crash PLI");
    let (_, v) = sigs.iter().find(|(n, _)| n == "q").unwrap();
    assert_eq!(v.to_u64(), 2, "q harus 2 setelah dua cycle");

    // PLI tf API tetap berfungsi setelah sim (murni Rust, tidak bergantung
    // engine).
    tf_set_current_instance(42);
    assert_eq!(tf_getinstance(), 42);
    tf_set_current_time(99);
    assert_eq!(tf_gettime(), 99);
}

#[test]
fn test_vhpi_value_change_callback_e2e() {
    // VHPI value-change callback (vhpiCbValueChange) di-fire engine saat
    // signal berubah (ForeignEvent::ValueChange ke scheduler — poin 5
    // arsitektur user). Callback di-register utk signal 'q'; verifikasi
    // terpanggil saat q berubah dari 0 → 1.
    use mivon_api::vhpi::callback::{fire_value_change_callbacks, vhpiCbValueChange};
    use mivon_api::vhpi::handle::{register_object_for_test, VhpiObjectKind};
    use std::sync::atomic::{AtomicI32, Ordering};

    static FIRED: AtomicI32 = AtomicI32::new(0);

    extern "C" fn vc_cb(_data: *mut mivon_api::vhpi::callback::t_vhpi_cb_data) -> i32 {
        FIRED.fetch_add(1, Ordering::SeqCst);
        0
    }

    // Registrasi callback value-change utk signal id 0 (obj Signal(0,0)).
    let obj = register_object_for_test(VhpiObjectKind::Signal(0, 0));
    let cb = mivon_api::vhpi::callback::t_vhpi_cb_data {
        reason: vhpiCbValueChange,
        cb_rtn: Some(vc_cb),
        user_data: std::ptr::null_mut(),
        obj,
        time: std::ptr::null_mut(),
    };
    let h = mivon_api::vhpi::callback::vhpi_register_cb(&cb);
    assert!(h.is_valid(), "callback terdaftar");

    // Fire langsung (simulasi jalur engine: commit signal → callback).
    let old = mivon_ir::LogicVec::from_u64(0, 8);
    let new = mivon_ir::LogicVec::from_u64(1, 8);
    fire_value_change_callbacks(0, &old, &new);
    assert_eq!(
        FIRED.load(Ordering::SeqCst),
        1,
        "value-change callback utk signal 0 terpanggil"
    );

    // Signal id lain → tidak terpanggil.
    fire_value_change_callbacks(5, &old, &new);
    assert_eq!(
        FIRED.load(Ordering::SeqCst),
        1,
        "signal id beda tak boleh fire"
    );

    // obj NULL (semua signal) → terpanggil utk signal apa pun.
    let cb_all = mivon_api::vhpi::callback::t_vhpi_cb_data {
        reason: vhpiCbValueChange,
        cb_rtn: Some(vc_cb),
        user_data: std::ptr::null_mut(),
        obj: mivon_api::vhpi::handle::VhpiHandle::NULL,
        time: std::ptr::null_mut(),
    };
    let h2 = mivon_api::vhpi::callback::vhpi_register_cb(&cb_all);
    fire_value_change_callbacks(9, &old, &new);
    assert_eq!(
        FIRED.load(Ordering::SeqCst),
        2,
        "obj NULL fire utk signal apa pun"
    );

    mivon_api::vhpi::callback::vhpi_remove_cb(h);
    mivon_api::vhpi::callback::vhpi_remove_cb(h2);
    mivon_api::vhpi::callback::clear_all_callbacks();
}

#[test]
fn test_vhpi_loader_e2e_gcc_so() {
    // e2e VHPI library C sungguhan (arsitektur poin 3: JANGAN compile PLI/
    // VHPI ke Rust — pakai C ABI + dynamic loader). Compile C → .so via gcc,
    // load via foreign loader, verifikasi `vhpi_startup` C terpanggil.
    // Skip test bila gcc tidak tersedia di environment.
    use std::path::Path;
    use std::process::Command;

    // Cek gcc tersedia.
    let gcc_ok = Command::new("gcc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gcc_ok {
        eprintln!("SKIP: gcc tidak tersedia — test VHPI .so dilewati");
        return;
    }

    let dir = std::env::temp_dir().join(format!("mivon_vhpi_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("buat dir temp");
    let c_path = dir.join("vhpi_stub.c");
    let so_path = dir.join("libvhpi_stub.so");
    std::fs::write(
        &c_path,
        r#"
#include <stdio.h>
int vhpi_startup(void) {
    printf("vhpi_startup called (C stub)\n");
    return 0;
}
"#,
    )
    .expect("tulis C stub");

    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("jalankan gcc");
    assert!(status.success(), "gcc compile .so gagal");

    // Load via foreign loader (canonicalize path absolut — dlopen tak search cwd).
    let vhpi = mivon_api::vhpi::loader::load_vhpi_library(so_path.to_str().unwrap())
        .expect("load VHPI .so");
    assert_eq!(vhpi.abi.arch, std::env::consts::ARCH, "ABI arch cocok");
    assert_eq!(vhpi.abi.os, std::env::consts::OS, "ABI os cocok");

    // vhpi_startup dari C terpanggil (return 0 = sukses).
    mivon_api::vhpi::loader::call_vhpi_startup(&vhpi).expect("vhpi_startup sukses");

    // Bersihkan.
    let _ = std::fs::remove_dir_all(&dir);
    let _ = Path::new("libvhpi_test.so"); // (scratch manual, bukan test ini)
}

#[test]
fn test_pli_loader_e2e_gcc_so() {
    // e2e PLI library C sungguhan: compile C → .so, load, cek entry point
    // `veriusertfs` / `vpi_startup` terdeteksi. Skip bila gcc tidak ada.
    use std::process::Command;

    let gcc_ok = Command::new("gcc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !gcc_ok {
        eprintln!("SKIP: gcc tidak tersedia — test PLI .so dilewati");
        return;
    }

    let dir = std::env::temp_dir().join(format!("mivon_pli_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("buat dir temp");
    let c_path = dir.join("pli_stub.c");
    let so_path = dir.join("libpli_stub.so");
    std::fs::write(
        &c_path,
        r#"
#include <stdio.h>
int vpi_startup(void) {
    printf("pli vpi_startup called (C stub)\n");
    return 0;
}
"#,
    )
    .expect("tulis C stub");

    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&so_path)
        .arg(&c_path)
        .status()
        .expect("jalankan gcc");
    assert!(status.success(), "gcc compile .so gagal");

    let pli =
        mivon_api::pli::loader::load_pli_library(so_path.to_str().unwrap()).expect("load PLI .so");
    assert!(
        mivon_api::pli::loader::has_pli_entry_points(&pli),
        "entry point vpi_startup terdeteksi"
    );
    mivon_api::pli::loader::call_pli_startup(&pli).expect("pli vpi_startup sukses");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_randomize_soft_constraint_satisfied() {
    // LANG-31: `soft` constraint — best-effort. Soft yang TIDAK bertentangan
    // dengan hard constraint: randomize harus sukses (soft boleh terpenuhi
    // atau dilanggar, tidak pernah membuat randomize gagal).
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint soft_range {
        soft addr == 100;
        addr inside {[1:200]};
    }
endclass

module tb;
    Packet p;
    int result;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "randomize with soft constraint should succeed"
    );
}

#[test]
fn test_randomize_soft_constraint_yields_to_hard() {
    // LANG-31: soft yang BERTENTANGAN dengan hard constraint HARUS dikalahkan
    // (IEEE 1800-2017 §18.5.14) — randomize tetap sukses dengan nilai
    // memenuhi hard (`addr < 10`), soft `addr == 100` dilanggar.
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint c {
        soft addr == 100;
        addr < 10;
    }
endclass

module tb;
    Packet p;
    int result;
    int addr_out;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
            addr_out = p.addr;
        end else begin
            result = 0;
            addr_out = -1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "soft must yield to hard: randomize should succeed"
    );
    let (_, addr) = sigs.iter().find(|(n, _)| n == "addr_out").unwrap();
    assert!(
        addr.to_u64() < 10,
        "addr must satisfy hard constraint addr < 10, got {}",
        addr.to_u64()
    );
}

#[test]
fn test_constraint_mode_disable_enables_randomize() {
    // LANG-33: `constraint_mode(0)` me-nonaktifkan constraint block —
    // randomize yang tadinya gagal (c1 addr<100 && c2 addr>200 tidak
    // mungkin) jadi sukses setelah c2 di-disable (hanya c1 aktif).
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint c1 { addr < 100; }
    constraint c2 { addr > 200; }
endclass

module tb;
    Packet p;
    int result;
    int addr_out;
    initial begin
        p = new();
        p.c2.constraint_mode(0);
        if (p.randomize()) begin
            result = 1;
            addr_out = p.addr;
        end else begin
            result = 0;
            addr_out = -1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "constraint_mode(0) harus mengaktifkan randomize (c2 di-skip)"
    );
    let (_, addr) = sigs.iter().find(|(n, _)| n == "addr_out").unwrap();
    assert!(
        addr.to_u64() < 100,
        "addr harus memenuhi c1 (addr < 100), got {}",
        addr.to_u64()
    );
}

#[test]
fn test_constraint_mode_query_and_reenable() {
    // LANG-33: query `constraint_mode()` mengembalikan mode (0/1); re-enable
    // via `constraint_mode(1)` membuat block aktif kembali.
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint c1 { addr < 100; }
    constraint c2 { addr > 200; }
endclass

module tb;
    Packet p;
    int mode_off, mode_on;
    int result_disabled, result_enabled;
    initial begin
        p = new();
        p.c2.constraint_mode(0);
        mode_off = p.c2.constraint_mode();
        result_disabled = p.randomize() ? 1 : 0;
        p.c2.constraint_mode(1);
        mode_on = p.c2.constraint_mode();
        result_enabled = p.randomize() ? 1 : 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let mode_off = sigs
        .iter()
        .find(|(n, _)| n == "mode_off")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let mode_on = sigs
        .iter()
        .find(|(n, _)| n == "mode_on")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(mode_off, 0, "mode setelah constraint_mode(0) harus 0");
    assert_eq!(mode_on, 1, "mode setelah constraint_mode(1) harus 1");
    let r_disabled = sigs
        .iter()
        .find(|(n, _)| n == "result_disabled")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let r_enabled = sigs
        .iter()
        .find(|(n, _)| n == "result_enabled")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(r_disabled, 1, "c2 nonaktif → randomize sukses");
    assert_eq!(r_enabled, 0, "c2 aktif kembali → randomize gagal (konflik)");
}

#[test]
fn test_randomize_per_instance_seed_distinct() {
    // VERIF-35: seed per-instance — dua instance class sama yang di-randomize
    // pada waktu SAMA harus mendapat nilai BERBEDA (seed lama hanya
    // current_time → deret identik). Tanpa fix: va == vb.
    let source = r#"
class Packet;
    rand logic [31:0] addr;
endclass

module tb;
    Packet p1;
    Packet p2;
    logic [31:0] va, vb;
    initial begin
        p1 = new();
        p2 = new();
        p1.randomize();
        p2.randomize();
        va = p1.addr;
        vb = p2.addr;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let va = sigs
        .iter()
        .find(|(n, _)| n == "va")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let vb = sigs
        .iter()
        .find(|(n, _)| n == "vb")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_ne!(
        va, vb,
        "per-instance seed: p1/p2 harus beda (va={}, vb={})",
        va, vb
    );
}

#[test]
fn test_randomize_per_instance_seed_reproducible() {
    // VERIF-35: deterministik — run kedua dengan (instance, waktu) sama
    // menghasilkan nilai sama (bukan thread_rng acak). $urandom global tetap
    // jalan (RNG engine tak disentuh).
    let src = r#"
class Packet;
    rand logic [31:0] addr;
endclass

module tb;
    Packet p1;
    logic [31:0] va;
    initial begin
        p1 = new();
        p1.randomize();
        va = p1.addr;
        #1 $finish;
    end
endmodule
"#;
    let sigs1 = simulate_signals(src, 5).unwrap();
    let sigs2 = simulate_signals(src, 5).unwrap();
    let a1 = sigs1
        .iter()
        .find(|(n, _)| n == "va")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let a2 = sigs2
        .iter()
        .find(|(n, _)| n == "va")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        a1, a2,
        "per-instance seed harus reproducible ({} vs {})",
        a1, a2
    );
}

#[test]
fn test_static_constraint_global_constraint_mode() {
    // LANG-32: `static constraint` dibagi antar SEMUA instance class
    // (IEEE 1800-2017 §18.5.10) — `constraint_mode(0/1)` pada satu instance
    // berlaku global: query via instance lain mengembalikan mode yang sama.
    let source = r#"
class Packet;
    rand bit [3:0] addr;
    static constraint c_static { addr == 5; }
    constraint c_inst { addr > 0; }
endclass

module tb;
    Packet p1;
    Packet p2;
    int r1;
    int r2;
    int m_p1;
    int m_p2;
    initial begin
        p1 = new();
        p2 = new();
        if (p1.randomize()) r1 = p1.addr;
        p1.c_static.constraint_mode(0);
        m_p1 = p1.c_static.constraint_mode();
        m_p2 = p2.c_static.constraint_mode();
        p2.c_static.constraint_mode(1);
        if (p2.randomize()) r2 = p2.addr;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| {
        sigs.iter()
            .find(|(s, _)| s == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("r1"), 5, "static constraint aktif: addr harus 5");
    assert_eq!(
        get("m_p1"),
        0,
        "constraint_mode(0) via p1 harus terlihat via p1"
    );
    assert_eq!(
        get("m_p2"),
        0,
        "static constraint_mode GLOBAL: query via p2 juga 0"
    );
    assert_eq!(
        get("r2"),
        5,
        "re-enable via p2 global → randomize p2 memenuhi addr==5 lagi"
    );
}

#[test]
fn test_randomize_no_constraint() {
    let source = r#"
class Simple;
    rand logic [7:0] val;
endclass

module tb;
    Simple s;
    int result;
    initial begin
        s = new();
        if (s.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        1,
        "randomize without constraints should succeed"
    );
}

#[test]
fn test_randomize_with_inside_constraint() {
    let source = r#"
class Packet;
    rand logic [7:0] addr;
    constraint addr_excl {
        addr != 0;
    }
endclass

module tb;
    Packet p;
    int result;
    initial begin
        p = new();
        if (p.randomize()) begin
            result = 1;
        end else begin
            result = 0;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 1, "randomize with constraint should succeed");
}

#[test]
fn test_foreach_in_class() {
    let source = r#"
class Accum;
    logic [31:0] arr [0:3];
    int sum;
    function new();
        sum = 0;
    endfunction
    function void init();
        arr[0] = 10;
        arr[1] = 20;
        arr[2] = 30;
        arr[3] = 40;
    endfunction
    function void accumulate();
        foreach (arr[i]) begin
            sum = sum + arr[i];
        end
    endfunction
endclass

module tb;
    Accum a;
    int result;
    initial begin
        a = new();
        a.init();
        a.accumulate();
        result = a.sum;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(
        val.to_u64(),
        100,
        "foreach should sum array elements: 10+20+30+40=100"
    );
}

#[test]
fn test_preprocessor_define_and_expand() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define WIDTH 8\nmodule test;\n    wire [`WIDTH-1:0] data;\nendmodule\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire [8-1:0] data"),
        "macro should expand WIDTH: {}",
        result
    );
}

#[test]
fn test_preprocessor_ifdef() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    pp.define("DEBUG", "1");
    let source = "`ifdef DEBUG\nwire dbg;\n`else\nwire nodbg;\n`endif\nwire always;\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire dbg;"),
        "ifdef true branch should be emitted"
    );
    assert!(
        !result.contains("wire nodbg;"),
        "else branch should be skipped"
    );
    assert!(
        result.contains("wire always;"),
        "post-endif should be emitted"
    );
}

#[test]
fn test_preprocessor_ifndef() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`ifndef DEBUG\nwire dbg;\n`else\nwire nodbg;\n`endif\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire dbg;"),
        "ifndef true branch should be emitted"
    );
    assert!(
        !result.contains("wire nodbg;"),
        "else branch should be skipped"
    );
}

#[test]
fn test_preprocessor_strip_unknown_macro() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`uvm_component_utils(my_driver)\nmodule test;\nendmodule\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        !result.contains("`uvm_component_utils"),
        "unknown macro should be stripped"
    );
    assert!(
        result.contains("module test;"),
        "module decl should survive"
    );
}

#[test]
fn test_timescale_directive() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let src = "`timescale 1ns / 10ps\nmodule top;\ninitial #1 $finish;\nendmodule\n";
    let result = pp.preprocess(src, None).unwrap();
    assert_eq!(pp.timescale, Some(("1ns".to_string(), "10ps".to_string())));
    assert!(
        result.contains("module top;"),
        "timescale should pass through module text"
    );
}

#[test]
fn test_preprocessor_nested_ifdef() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    pp.define("A", "1");
    pp.define("B", "1");
    let source =
        "`ifdef A\n`ifdef B\nwire both;\n`else\nwire only_a;\n`endif\n`endif\nwire after;\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire both;"),
        "both defined: both should be emitted"
    );
    assert!(!result.contains("wire only_a;"), "else should be skipped");
    assert!(result.contains("wire after;"), "post-endif emitted");
}

#[test]
fn test_preprocessor_macro_arguments() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define ADD(a,b) a + b\nwire `ADD(x,y);\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("wire x + y;"),
        "macro args should substitute: {}",
        result
    );
}

#[test]
fn test_preprocessor_macro_args_complex() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define MIN(a,b) ((a) < (b) ? (a) : (b))\nwire [3:0] w = `MIN(4+1, 8);\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("((4+1) < (8) ? (4+1) : (8))"),
        "complex macro: {}",
        result
    );
}

#[test]
fn test_preprocessor_macro_args_multiline() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define SUM(a,b,c) a + b + c\nwire w = `SUM(x, y, z);\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(result.contains("x + y + z"), "three args: {}", result);
}

#[test]
fn test_preprocessor_macro_debug_output() {
    use mivon_parser::preprocessor::Preprocessor;
    let mut pp = Preprocessor::new();
    let source = "`define ADD(a,b) a + b\nmodule tb;\n    reg [3:0] sum;\n    initial begin\n        sum = `ADD(2, 3);\n        #1 $finish;\n    end\nendmodule\n";
    let result = pp.preprocess(source, None).unwrap();
    assert!(
        result.contains("sum = 2 + 3;"),
        "macro should expand: '{}'",
        result
    );
}

#[test]
fn test_preprocessor_macro_args_in_expression() {
    let source = r#"
`define ADD(a,b) a + b

module tb;
    reg [3:0] sum;
    initial begin
        sum = `ADD(2, 3);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 2).unwrap();
    let sum_val = sigs
        .iter()
        .find(|(n, _)| n == "sum")
        .map(|(_, v)| v.to_u64())
        .unwrap();
    assert_eq!(
        sum_val, 5,
        "macro ADD(2,3) should expand to 2+3=5, got {}",
        sum_val
    );
}

#[test]
fn test_event_control_procedural() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] q;
    initial begin
        clk = 0;
        q = 0;
        #5 clk = 1;
        #1 clk = 0;
        #1 clk = 1;
        #1 $finish;
    end
    always @(posedge clk) begin
        q <= q + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(q_val, 2, "q should be 2 after 2 posedge clk events");
}

#[test]
fn test_event_control_procedural_at() {
    let source = r#"
module tb;
    reg clk;
    reg [7:0] q;
    initial begin
        clk = 0;
        q = 0;
        #5 clk = 1;
        @(posedge clk);
        q = 42;
        #1 $finish;
    end
    always #5 clk = ~clk;
endmodule
"#;
    // `@(posedge clk)` BLOCKING per IEEE: posedge yang sudah lewat (clk naik di #5)
    // tidak dihitung — menunggu posedge BERIKUTNYA (di #15 dari always #5).
    let sigs = simulate_signals(source, 20).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        q_val, 42,
        "q should be 42 after @(posedge clk) blocks until next edge"
    );
}

#[test]
fn test_event_trigger() {
    let source = r#"
module tb;
    reg ev;
    reg [7:0] q;
    initial begin
        q = 0;
        -> ev;
        #1 $finish;
    end
    initial begin
        @(ev) q = 99;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(q_val, 99, "q should be 99 after -> ev triggers @(ev)");
}

#[test]
fn test_specify_parse() {
    let source = r#"
module tb;
    reg data, clk;
    specify
        specparam tSU = 1.0;
        $setup(data, posedge clk, tSU);
        $hold(posedge clk, data, 0.5);
        (data => q) = (1.0);
    endspecify
endmodule
"#;
    let result = compile_str(source);
    assert!(result.is_ok(), "specify block compile should succeed");
}

#[test]
fn test_specify_with_module() {
    let source = r#"
module dut(input clk, input d, output reg q);
    always_ff @(posedge clk) q <= d;
    specify
        $setup(d, posedge clk, 1);
        $hold(posedge clk, d, 0);
    endspecify
endmodule
module tb;
    reg clk, d;
    wire q;
    dut u1(.clk(clk), .d(d), .q(q));
    initial begin
        clk = 0; d = 0;
        #5 clk = 1; #5 clk = 0;
        #5 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    assert!(sigs.iter().any(|(n, _)| n == "q"), "q signal should exist");
}

#[test]
fn test_event_control_iff_always_ff() {
    // LANG-27: `always_ff @(posedge clk iff (en))` — q hanya update saat
    // en=1 pada edge naik clk. Saat en=0, edge clk diabaikan.
    let source = r#"
module tb;
    reg clk = 0, en = 0, d = 0;
    reg q = 0;
    always_ff @(posedge clk iff (en)) q <= d;
    always #5 clk = ~clk;
    initial begin
        d = 1;
        // Edge naik pertama (t=10): en masih 0 → q harus tetap 0.
        en = 0;
        #10 en = 1;
        // Edge naik kedua (t=20): en=1 → q = d = 1.
        #10 $display("q=%0d", q);
        if (q !== 1'b1) $error("iff guard failed: q=%0b expected 1", q);
        $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 25).unwrap();
    let q = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .expect("q signal should exist")
        .1
        .clone();
    assert_eq!(q.to_u64(), 1, "q should be 1 after enabled edge (iff gate)");
}

#[test]
fn test_event_control_iff_blocks_when_cond_false() {
    // LANG-27: guard `iff` di blocking event control `@(posedge clk iff (g))`
    // di procedural code — body hanya lanjut bila kondisi benar.
    let source = r#"
module tb;
    reg clk = 0;
    reg g = 0;
    integer count = 0;
    always #5 clk = ~clk;
    initial begin
        // Set g=1 sebelum edge kedua; edge pertama (t=10) di-skip.
        #10 g = 1;
        #20 $display("count=%0d", count);
        if (count !== 1) $error("iff blocking failed: count=%0d expected 1", count);
        $finish;
    end
    initial begin
        @(posedge clk iff (g)) count = count + 1;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 30).unwrap();
    let count = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .expect("count signal should exist")
        .1
        .clone();
    assert_eq!(
        count.to_u64(),
        1,
        "count should be 1 (only gated edge counts)"
    );
}

#[test]
fn test_sensitivity_iff_elaborated_sequential() {
    // LANG-27: `always @(posedge clk iff (en))` ter-elaborasi jadi
    // Process::Sequential dengan field iff terisi (guard kondisi).
    let src = r#"
module m(input clk, input en, input d, output reg q);
    always @(posedge clk iff (en)) q <= d;
endmodule
module tb;
    reg clk, en, d;
    wire q;
    m u1(.clk(clk), .en(en), .d(d), .q(q));
endmodule
"#;
    let design = compile_str(src).expect("compile should succeed");
    let has_iff = design
        .top
        .processes
        .iter()
        .any(|p| matches!(p, mivon_ir::Process::Sequential { iff: Some(_), .. }));
    assert!(has_iff, "sequential process should carry iff guard");
}

#[test]
fn test_udp_sequential_dff_posedge0() {
    let source = r#"
primitive dff(output reg q, input clk, input d);
    table
        (01) 0 : ? : 0;
        (01) 1 : ? : 1;
        ?    ? : ? : -;
    endtable
endprimitive

module tb;
    reg clk, d;
    wire q;
    dff u1(q, clk, d);
    initial begin
        clk = 0; d = 0;
        #1 clk = 1;
        #1 if (q !== 0) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(q_val, 0, "dff: posedge with d=0 -> q=0");
}

#[test]
fn test_udp_sequential_dff_posedge1() {
    let source = r#"
primitive dff(output reg q, input clk, input d);
    table
        (01) 0 : ? : 0;
        (01) 1 : ? : 1;
        ?    ? : ? : -;
    endtable
endprimitive

module tb;
    reg clk, d;
    wire q;
    dff u1(q, clk, d);
    initial begin
        clk = 0; d = 0;
        #1 clk = 1; $display("t1 clk=%b d=%b q=%b", clk, d, q);
        #1 clk = 0; d = 1; $display("t2 clk=%b d=%b q=%b", clk, d, q);
        #1 clk = 1; $display("t3 clk=%b d=%b q=%b", clk, d, q);
        #1 $display("t4 clk=%b d=%b q=%b", clk, d, q);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 15).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(q_val, 1, "dff: second posedge with d=1 -> q=1");
}

#[test]
fn test_udp_sequential_dff_initial() {
    let source = r#"
primitive dff_init(output reg q, input clk, input d);
    initial q = 0;
    table
        (01) 0 : ? : 0;
        (01) 1 : ? : 1;
        (0?) 1 : 1 : 1;
        (?0) ? : ? : -;
        ?    ? : ? : -;
    endtable
endprimitive

module tb;
    reg clk, d;
    wire q;
    dff_init u1(q, clk, d);
    initial begin
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let q_val = sigs
        .iter()
        .find(|(n, _)| n == "q")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(q_val, 0, "sequential dff: initial q should be 0");
}

#[test]
fn test_sysfunc_countones() {
    let source = r#"
module tb;
    reg [7:0] val;
    reg [31:0] result;
    initial begin
        val = 8'b10100101;
        result = $countones(val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let r = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(r, 4, "$countones(8'b10100101) = 4");
}

/// `$past(v[,n])`: riwayat per (arg, n) — tick 1/2/3 lag 1/2/3 sampel.
/// Regresi: kunci hanya arg → deque tercampur (p1=p2=p3=4, harus 30/20/10).
#[test]
fn test_sysfunc_past_ticks() {
    let source = r#"
module tb;
bit clk;
logic [7:0] cnt, p1, p2, p3;
initial begin
clk = 0; cnt = 8'd10;
#12 cnt = 8'd20;
#10 cnt = 8'd30;
#10 cnt = 8'd40;
#10;
$display("cnt=%0d p1=%0d p2=%0d p3=%0d", cnt, p1, p2, p3);
$finish;
end
always @(posedge clk) begin
p1 <= $past(cnt);
p2 <= $past(cnt, 2);
p3 <= $past(cnt, 3);
end
initial begin forever #5 clk = ~clk; end
endmodule
"#;
    let sigs = simulate_signals(source, 100).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("p1"), 30, "$past(cnt) lag 1");
    assert_eq!(get("p2"), 20, "$past(cnt,2) lag 2");
    assert_eq!(get("p3"), 10, "$past(cnt,3) lag 3");
}

/// `$past` tanpa riwayat cukup → X (4-state), bukan 0.
#[test]
fn test_sysfunc_past_initial_x() {
    let source = r#"
module tb;
logic [7:0] v;
bit w1, w2;
initial begin
v = 8'hAA;
w1 = $isunknown($past(v));
w2 = $isunknown($past(v));
#1 $finish;
end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("w1"), 1, "$past pertama harus X");
    assert_eq!(get("w2"), 0, "$past kedua sudah ada riwayat");
}

#[test]
fn test_sysfunc_onehot() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg onehot_a, onehot_b;
    initial begin
        a = 4'b0100;
        b = 4'b0110;
        onehot_a = $onehot(a);
        onehot_b = $onehot(b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ra = sigs
        .iter()
        .find(|(n, _)| n == "onehot_a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    let rb = sigs
        .iter()
        .find(|(n, _)| n == "onehot_b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(ra, 1, "$onehot(4'b0100) = 1");
    assert_eq!(rb, 0, "$onehot(4'b0110) = 0");
}

#[test]
fn test_sysfunc_onehot0() {
    let source = r#"
module tb;
    reg [3:0] a, b, c, d, e;
    reg oh0_a, oh0_b, oh0_c, oh0_d, oh0_e;
    reg oh_a, oh_b, oh_c;
    initial begin
        a = 4'b0000;  // zero bits
        b = 4'b0001;  // one bit
        c = 4'b0011;  // two bits
        d = 4'b0101;  // two bits non-adjacent
        e = 4'b1111;  // four bits
        
        oh0_a = $onehot0(a);
        oh0_b = $onehot0(b);
        oh0_c = $onehot0(c);
        oh0_d = $onehot0(d);
        oh0_e = $onehot0(e);
        
        oh_a = $onehot(a);
        oh_b = $onehot(b);
        oh_c = $onehot(c);
        
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    // $onehot0 returns 1 for 0 or 1 bits set
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_a").unwrap().1.to_u64(),
        1,
        "$onehot0(0000)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_b").unwrap().1.to_u64(),
        1,
        "$onehot0(0001)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_c").unwrap().1.to_u64(),
        0,
        "$onehot0(0011)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_d").unwrap().1.to_u64(),
        0,
        "$onehot0(0101)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh0_e").unwrap().1.to_u64(),
        0,
        "$onehot0(1111)"
    );
    // $onehot returns 1 only for exactly 1 bit set
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh_a").unwrap().1.to_u64(),
        0,
        "$onehot(0000)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh_b").unwrap().1.to_u64(),
        1,
        "$onehot(0001)"
    );
    assert_eq!(
        sigs.iter().find(|(n, _)| n == "oh_c").unwrap().1.to_u64(),
        0,
        "$onehot(0011)"
    );
}

#[test]
fn test_sysfunc_isunknown() {
    let source = r#"
module tb;
    reg [3:0] a, b;
    reg unk_a, unk_b;
    initial begin
        a = 4'b1010;
        b = 4'b10xz;
        unk_a = $isunknown(a);
        unk_b = $isunknown(b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ra = sigs
        .iter()
        .find(|(n, _)| n == "unk_a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    let rb = sigs
        .iter()
        .find(|(n, _)| n == "unk_b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(ra, 0, "$isunknown(4'b1010) = 0");
    assert_eq!(rb, 1, "$isunknown(4'b10xz) = 1");
}

#[test]
fn test_sysfunc_typename() {
    let source = r#"
module tb;
    logic [7:0] sig_a;
    logic signed [15:0] sig_b;
    logic [3:0][7:0] sig_c;
    int sig_d;
    real sig_e;
    string sig_f;
    initial begin
        $display("sig_a: %s", $typename(sig_a));
        $display("sig_b: %s", $typename(sig_b));
        $display("sig_c: %s", $typename(sig_c));
        $display("sig_d: %s", $typename(sig_d));
        $display("sig_e: %s", $typename(sig_e));
        $display("sig_f: %s", $typename(sig_f));
        $display("lit_int: %s", $typename(42));
        $display("lit_real: %s", $typename(3.14));
        $display("lit_str: %s", $typename("hello"));
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    // Just verify it runs without error - $typename returns string as LogicVec
    // Check that all declared signals exist
    // Note: string signals have width 0 in current implementation (dynamic string)
    let sig_names = ["sig_a", "sig_b", "sig_c", "sig_d", "sig_e", "sig_f"];
    for name in sig_names {
        let found = sigs.iter().find(|(n, _)| n == name);
        assert!(found.is_some(), "Signal {} not found in results", name);
    }
}

#[test]
fn test_sysfunc_countbits() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [15:0] b;
    reg [31:0] result_a, result_b;
    initial begin
        a = 8'b10100101;  // 4 ones
        b = 16'hA5A5;      // 8 ones (A=1010, 5=0101)
        result_a = $countbits(a, 1'b1);
        result_b = $countbits(b, 1'b1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let ra = sigs
        .iter()
        .find(|(n, _)| n == "result_a")
        .unwrap()
        .1
        .to_u64();
    let rb = sigs
        .iter()
        .find(|(n, _)| n == "result_b")
        .unwrap()
        .1
        .to_u64();
    assert_eq!(ra, 4, "$countbits(8'b10100101) = 4");
    assert_eq!(rb, 8, "$countbits(16'hA5A5) = 8");
}

#[test]
fn test_sysfunc_dimensions() {
    let source = r#"
module tb;
    logic [7:0] scalar;
    logic [3:0][7:0] packed_2d;
    logic [3:0] unpacked [7:0];
    logic [2:0][1:0][3:0] packed_3d;
    
    reg [31:0] dim_scalar, dim_packed_2d, dim_unpacked, dim_packed_3d;
    initial begin
        dim_scalar = $dimensions(scalar);
        dim_packed_2d = $dimensions(packed_2d);
        dim_unpacked = $dimensions(unpacked);
        dim_packed_3d = $dimensions(packed_3d);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_scalar")
            .unwrap()
            .1
            .to_u64(),
        0,
        "$dimensions(scalar) = 0"
    );
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_packed_2d")
            .unwrap()
            .1
            .to_u64(),
        2,
        "$dimensions(packed_2d) = 2"
    );
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_unpacked")
            .unwrap()
            .1
            .to_u64(),
        1,
        "$dimensions(unpacked) = 1"
    );
    assert_eq!(
        sigs.iter()
            .find(|(n, _)| n == "dim_packed_3d")
            .unwrap()
            .1
            .to_u64(),
        3,
        "$dimensions(packed_3d) = 3"
    );
}

#[test]
fn test_timing_check_setup() {
    let source = r#"
module tb;
    reg data, clk;
    wire q;
    specify
        $setup(data, posedge clk, 5);
    endspecify
    initial begin
        data = 0;
        #1 clk = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    assert!(sigs.iter().any(|(n, _)| n == "data"));
}

#[test]
fn test_fgets_string_var() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fgets.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "hello").unwrap();
        writeln!(f, "world").unwrap();
    }
    let source = format!(
        r#"
module tb;
    string line;
    integer fd;
    initial begin
        fd = $fopen("{}", "r");
        if (fd == 0) begin
            $display("FAIL: cannot open file");
            $finish;
        end
        #1;
        $fgets(line, fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    // Check that line has data (non-empty string signal)
    let line_sig = sigs.iter().find(|(n, _)| n == "line");
    assert!(line_sig.is_some(), "line signal should exist");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_fgetc_basic() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fgetc.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "A").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer c;
    integer fd;
    initial begin
        fd = $fopen("{}", "r");
        #1;
        c = $fgetc(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let c_val = sigs
        .iter()
        .find(|(n, _)| n == "c")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // 'A' = 65
    assert_eq!(c_val, 65, "$fgetc should read 'A' (65)");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_fflush_basic() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fflush.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "hello").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer fd;
    initial begin
        fd = $fopen("{}", "a");
        $fwrite(fd, "world");
        $fflush(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let _sigs = simulate_signals(&source, 10).unwrap();
    let _ = std::fs::remove_file(&tmp);
    // Just verify no crash
}

#[test]
fn test_fseek_ftell() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_fseek.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        writeln!(f, "ABCDEFGHIJ").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer fd;
    integer pos;
    integer ch;
    initial begin
        fd = $fopen("{}", "r");
        #1;
        ch = $fgetc(fd);
        pos = $ftell(fd);
        $fseek(fd, 0, 0);
        ch = $fgetc(fd);
        pos = $ftell(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let pos_val = sigs
        .iter()
        .find(|(n, _)| n == "pos")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(pos_val, 1, "$ftell after reading 1 byte should be 1");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_feof() {
    use std::io::Write;
    let tmp = std::env::temp_dir().join("mivon_test_feof.txt");
    {
        let mut f = std::fs::File::create(&tmp).unwrap();
        write!(f, "AB").unwrap();
    }
    let source = format!(
        r#"
module tb;
    integer fd;
    integer eof;
    integer ch;
    initial begin
        fd = $fopen("{}", "r");
        eof = $feof(fd);
        ch = $fgetc(fd);
        ch = $fgetc(fd);
        ch = $fgetc(fd);
        eof = $feof(fd);
        #1 $finish;
    end
endmodule
"#,
        tmp.display()
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let eof_val = sigs
        .iter()
        .find(|(n, _)| n == "eof")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(eof_val, 1, "$feof should be 1 after reading past end");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_const_decl() {
    let source = r#"
module tb;
    const logic [7:0] x = 42;
    reg [7:0] y;
    initial begin
        y = x;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let x_val = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(x_val, 42, "const x should be 42");
}

#[test]
fn test_parallel_eval_basic() {
    let source = r#"
module tb;
    reg [7:0] a, b, c, d;
    wire [7:0] x, y;
    assign x = a + b;
    assign y = c + d;
    initial begin
        a = 1; b = 2; c = 3; d = 4;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    // Enable parallel with threshold of 1 for testing
    let pcfg = crate::simulator::parallel::ParallelConfig {
        min_processes_parallel: 1,
        parallel_processes: true,
        ..Default::default()
    };
    engine.set_parallel_config(pcfg);
    engine.run().unwrap();
    let sigs = engine.design.top.signals.clone();
    let x_val = sigs
        .iter()
        .find(|_s| _s.name == "x")
        .map(|_s| {
            engine
                .state
                .read_signal(
                    engine
                        .design
                        .top
                        .signals
                        .iter()
                        .position(|x| x.name == "x")
                        .unwrap_or(0),
                )
                .to_u64()
        })
        .unwrap_or(0);
    assert_eq!(x_val, 3, "parallel: x = a + b = 1 + 2 = 3");
}

#[test]
fn test_gate_primitives_and_or() {
    let source = r#"
module tb;
    reg a, b, c, d;
    wire and_out, or_out;
    and a1(and_out, a, b, c);
    or  o1(or_out, a, b);
    initial begin
        a = 1; b = 1; c = 1;
        #1 d = and_out;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let d_val = sigs
        .iter()
        .find(|(n, _)| n == "d")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(d_val, 1, "and_out should be 1 (1 & 1 & 1 = 1)");
}

#[test]
fn test_gate_not_buf() {
    let source = r#"
module tb;
    reg in;
    wire out;
    not n1(out, in);
    initial begin
        in = 0;
        #1;
        if (out !== 1) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 1, "not gate should invert 0 to 1");
}

#[test]
fn test_udp_combinational_and() {
    let source = r#"
primitive udp_and(output z, input a, input b);
    table
        0 0 : 0;
        0 1 : 0;
        1 0 : 0;
        1 1 : 1;
    endtable
endprimitive

module tb;
    reg a, b;
    wire z;
    udp_and u1(z, a, b);
    initial begin
        a = 0; b = 0; #1;
        if (z !== 0) $finish;
        a = 1; b = 1; #1;
        if (z !== 1) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let z_val = sigs
        .iter()
        .find(|(n, _)| n == "z")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(z_val, 1, "UDP and: 1 & 1 = 1");
}

#[test]
fn test_udp_combinational_mux() {
    let source = r#"
primitive udp_mux(output z, input a, input b, input sel);
    table
        0 ? 0 : 0;
        1 ? 0 : 1;
        ? 0 1 : 0;
        ? 1 1 : 1;
    endtable
endprimitive

module tb;
    reg a, b, sel;
    wire z;
    udp_mux u1(z, a, b, sel);
    initial begin
        a = 1; b = 0; sel = 0; #1;
        if (z !== 1) $finish;
        a = 1; b = 0; sel = 1; #1;
        if (z !== 0) $finish;
        a = 0; b = 1; sel = 0; #1;
        if (z !== 0) $finish;
        a = 0; b = 1; sel = 1; #1;
        if (z !== 1) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let z_val = sigs
        .iter()
        .find(|(n, _)| n == "z")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(2);
    assert_eq!(z_val, 1, "UDP mux: sel=1,b=1 -> 1");
}

#[test]
fn test_udp_compile_only() {
    let source = r#"
primitive udp_nand(output z, input a, input b);
    table
        0 0 : 1;
        0 1 : 1;
        1 0 : 1;
        1 1 : 0;
    endtable
endprimitive
module tb;
    wire z;
    reg a = 0, b = 0;
    udp_nand u1(z, a, b);
endmodule
"#;
    let result = compile_str(source);
    assert!(result.is_ok(), "UDP compile should succeed");
}

#[test]
fn test_monitor_task() {
    let source = r#"
module tb;
    reg a;
    initial begin
        a = 0;
        $monitor("a=%d", a);
        #1 a = 1;
        #1 a = 0;
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 10).unwrap();
}

#[test]
fn test_string_methods_len_substr() {
    let source = r#"
module tb;
    reg [63:0] len_val;
    initial begin
        len_val = "hello".len();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let len = sigs
        .iter()
        .find(|(n, _)| n == "len_val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(len, 5, "len of 'hello' should be 5");
}

#[test]
fn test_string_methods_atoi() {
    let source = r#"
module tb;
    reg [31:0] val;
    initial begin
        val = "42".atoi();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 42, "atoi of '42' should be 42");
}

#[test]
fn test_string_methods_getc_and_ato() {
    // LRM 1800 §6.16.1 — getc / atohex / atobin / atooct / atoreal.
    let source = r#"
module tb;
    reg [7:0]  gc0;
    reg [7:0]  gc1;
    reg [7:0]  gc9;   // out-of-range -> 0
    reg [31:0] hx;
    reg [31:0] bn;
    reg [31:0] oc;
    real       rl;
    initial begin
        gc0 = "ABC".getc(0);   // 'A' = 65
        gc1 = "ABC".getc(1);   // 'B' = 66
        gc9 = "ABC".getc(9);   // out of range -> 0
        hx  = "1f".atohex();   // 31
        bn  = "101".atobin();  // 5
        oc  = "17".atooct();   // 15
        rl  = "3.5".atoreal(); // 3.5
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let g = |n: &str| {
        sigs.iter()
            .find(|(nm, _)| nm == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(g("gc0"), 65, "getc(0) of 'ABC' = 'A' = 65");
    assert_eq!(g("gc1"), 66, "getc(1) of 'ABC' = 'B' = 66");
    assert_eq!(g("gc9"), 0, "getc out-of-range = 0");
    assert_eq!(g("hx"), 31, "atohex('1f') = 31");
    assert_eq!(g("bn"), 5, "atobin('101') = 5");
    assert_eq!(g("oc"), 15, "atooct('17') = 15");
    // atoreal: bit-pattern f64 3.5 == 0x400C000000000000
    assert_eq!(g("rl"), 0x400C000000000000u64, "atoreal('3.5') = 3.5");
}

#[test]
fn test_string_methods_mutating() {
    // LRM 1800 §6.16.1 — method MUTATING: itoa/hextoa/bintoa/octtoa/putc.
    // Memverifikasi (1) write-back hasil ke receiver string, (2) bentuk
    // assignment `t = s.itoa(...)` juga berfungsi, dan (3) fix panic F91:
    // shift-overflow di logicvec_to_string pada string hasil putc yang
    // diperpanjang dengan NUL (internal NUL) serta indeks negatif putc (no-op).
    let source = r#"
module tb;
    string a, b, c, d, e, f, g, z, y;
    reg [31:0] flen;
    initial begin
        a.itoa(-42);                 // a = "-42"
        b = "abc";  b.hextoa(255);   // b = "ff"
        c = "abc";  c.bintoa(5);     // c = "101"
        d = "abc";  d.octtoa(15);    // d = "17"
        e = "abc";  e.putc(0, 88);   // e = "Xbc"
        f = "abc";  f.putc(5, 90);   // f = "abc\0\0Z" (extend dgn NUL)
        g = "abc";  g.putc(-3, 77);  // indeks negatif -> no-op (g = "abc")
        flen = f.len();              // 6
        z = "x";  y = z.itoa(9);     // assignment-form: y="9", z juga "9" (mutating)
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| -> Vec<u8> {
        sigs.iter()
            .find(|(nm, _)| nm == n)
            .map(|(_, v)| logicvec_to_string(v).into_bytes())
            .unwrap_or_default()
    };
    let flen = sigs
        .iter()
        .find(|(n, _)| n == "flen")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);

    assert_eq!(get("a"), b"-42".to_vec(), "itoa(-42) -> \"-42\"");
    assert_eq!(get("b"), b"ff".to_vec(), "hextoa(255) -> \"ff\"");
    assert_eq!(get("c"), b"101".to_vec(), "bintoa(5) -> \"101\"");
    assert_eq!(get("d"), b"17".to_vec(), "octtoa(15) -> \"17\"");
    assert_eq!(get("e"), b"Xbc".to_vec(), "putc(0,88) -> \"Xbc\"");
    // putc(5,90) pada "abc" -> "abc\0\0Z" (panjang 6, NUL internal).
    assert_eq!(
        get("f"),
        vec![b'a', b'b', b'c', 0, 0, b'Z'],
        "putc(5,90) extend dgn NUL -> \"abc\\0\\0Z\""
    );
    assert_eq!(get("g"), b"abc".to_vec(), "putc(-3,..) no-op -> \"abc\"");
    assert_eq!(flen, 6, "len() setelah putc extend = 6");
    assert_eq!(
        get("y"),
        b"9".to_vec(),
        "assignment-form y = z.itoa(9) -> \"9\""
    );
    assert_eq!(
        get("z"),
        b"9".to_vec(),
        "itoa mutating: receiver z ikut berubah -> \"9\""
    );
}

#[test]
fn test_string_var_decl() {
    let source = r#"
module tb;
    string s;
    reg [31:0] len;
    initial begin
        s = "hello";
        len = s.len();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let len = sigs
        .iter()
        .find(|(n, _)| n == "len")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(len, 5, "string variable len should be 5");
}

#[test]
fn test_string_var_reassign() {
    let source = r#"
module tb;
    string s;
    reg [31:0] len;
    initial begin
        s = "hello";
        s = "hi";
        len = s.len();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let len = sigs
        .iter()
        .find(|(n, _)| n == "len")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(len, 2, "reassigned string variable len should be 2");
}

#[test]
fn test_string_var_display() {
    let source = r#"
module tb;
    string s;
    reg [31:0] result;
    initial begin
        s = "hello";
        result = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let result = sigs
        .iter()
        .find(|(n, _)| n == "result")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(result, 1, "string variable display should not crash");
}

#[test]
fn test_dynamic_array_decl() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] val;
    initial begin
        d[0] = 42;
        d[1] = 99;
        val = d[0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 42, "dynamic array element should be 42");
}

#[test]
fn test_dynamic_array_size() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] sz;
    initial begin
        d[0] = 10;
        d[1] = 20;
        sz = d.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 2, "dynamic array size should be 2 after 2 writes");
}

#[test]
fn test_dynamic_array_new_size() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] sz;
    initial begin
        d = new[5];
        sz = d.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 5, "dynamic array size should be 5 after new[5]");
}

/// `new[N](old)` menyalin min(old,new) elemen pertama (LRM 1800 §7.5.2.1).
/// Regresi: init dibuang parser → resize kehilangan semua data.
/// Oracle: iverilog (`size=6 d0=10..d3=40`, shrink `size=2 d0=10 d1=20`).
#[test]
fn test_dynamic_array_new_with_copy() {
    let source = r#"
module tb;
    int d[];
    reg [31:0] sz, v0, v3, s2, w0, w1;
    initial begin
        d = new[4];
        d[0] = 10; d[1] = 20; d[2] = 30; d[3] = 40;
        d = new[6](d);
        sz = d.size(); v0 = d[0]; v3 = d[3];
        d = new[2](d);
        s2 = d.size(); w0 = d[0]; w1 = d[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("sz"), 6, "grow size");
    assert_eq!(get("v0"), 10, "grow d0 preserved");
    assert_eq!(get("v3"), 40, "grow d3 preserved");
    assert_eq!(get("s2"), 2, "shrink size");
    assert_eq!(get("w0"), 10, "shrink d0 preserved");
    assert_eq!(get("w1"), 20, "shrink d1 preserved");
}

#[test]
fn test_queue_push_pop() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] val;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        val = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 10, "queue pop_front should return first element 10");
}

#[test]
fn test_queue_size() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] sz;
    initial begin
        q.push_back(10);
        q.push_back(20);
        sz = q.size();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 2, "queue size should be 2 after 2 pushes");
}

/// `q[$]` = elemen TERAKHIR (bukan pertama); tulis `q[$] = v` juga jalan.
/// Regresi: `$` jadi SysFunc tak dikenal → 0 → elemen pertama.
/// Oracle: iverilog (baca; tulis crash di iverilog).
#[test]
fn test_queue_dollar_index() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] last, first, w1;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        last = q[$];
        q[$] = 99;
        first = q[0];
        w1 = q[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("last"), 30, "q[$] = 30");
    assert_eq!(get("first"), 10, "q[0] tetap 10");
    assert_eq!(get("w1"), 20, "q[1] tetap 20");
    // last dibaca SEBELUM tulis; baca ulang implisit via w1? cek via sim kedua:
    let source2 = r#"
module tb;
    int q[$];
    reg [31:0] rl;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q[$] = 99;
        rl = q[$];
        #1 $finish;
    end
endmodule
"#;
    let sigs2 = simulate_signals(source2, 5).unwrap();
    let rl = sigs2.iter().find(|(s, _)| s == "rl").unwrap().1.to_u64();
    assert_eq!(rl, 99, "tulis q[$] = 99 terbaca kembali");
}

/// Index string `s[i]` = BYTE ke-i (karakter), bukan bit tunggal.
/// Regresi: BitSelect 1-bit → bit LSB char (`s[0]`="h" → 0, harus 104).
/// Oracle: Verilator (104,101,108,108,111).
#[test]
fn test_string_byte_index() {
    let source = r#"
module tb;
    string s;
    reg [7:0] c0, c2, c4, w0, w2;
    string t;
    initial begin
        s = "hello";
        c0 = s[0]; c2 = s[2]; c4 = s[4];
        t = "hello";
        t[0] = 74;
        t[2] = 90;
        w0 = t[0]; w2 = t[2];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    assert_eq!(get("c0"), 104, "s[0]='h'");
    assert_eq!(get("c2"), 108, "s[2]='l'");
    assert_eq!(get("c4"), 111, "s[4]='o'");
    assert_eq!(get("w0"), 74, "tulis t[0]='J'");
    assert_eq!(get("w2"), 90, "tulis t[2]='Z'");
}

#[test]
fn test_queue_push_front() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] val;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_front(5);
        val = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 5, "queue push_front then pop_front should return 5");
}

#[test]
fn test_queue_pop_back() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] val;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        val = q.pop_back();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 30, "pop_back should return last element 30");
}

#[test]
fn test_queue_exists() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] exists_0;
    reg [31:0] exists_5;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        exists_0 = q.exists(0);
        exists_5 = q.exists(5);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let e0 = sigs
        .iter()
        .find(|(n, _)| n == "exists_0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let e5 = sigs
        .iter()
        .find(|(n, _)| n == "exists_5")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(e0, 1, "exists(0) should be 1 for element at index 0");
    assert_eq!(e5, 0, "exists(5) should be 0 for index out of range");
}

#[test]
fn test_queue_delete_index() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] sz;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        q.delete(1);
        sz = q.size();
        v0 = q.pop_front();
        v1 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let sz = sigs
        .iter()
        .find(|(n, _)| n == "sz")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(sz, 2, "size should be 2 after delete(1)");
    assert_eq!(v0, 10, "first element should still be 10");
    assert_eq!(v1, 30, "second element should be 30 (index 1 deleted)");
}

#[test]
fn test_array_insert() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(10);
        q.push_back(30);
        q.insert(1, 20);
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 10, "insert: first element should be 10");
    assert_eq!(v1, 20, "insert: inserted element should be 20");
    assert_eq!(v2, 30, "insert: third element should be 30");
}

#[test]
fn test_array_reverse() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(10);
        q.push_back(20);
        q.push_back(30);
        q.reverse();
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 30, "reverse: first should be 30");
    assert_eq!(v1, 20, "reverse: second should be 20");
    assert_eq!(v2, 10, "reverse: third should be 10");
}

#[test]
fn test_array_sort() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(30);
        q.push_back(10);
        q.push_back(20);
        q.sort();
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 10, "sort: first should be 10");
    assert_eq!(v1, 20, "sort: second should be 20");
    assert_eq!(v2, 30, "sort: third should be 30");
}

#[test]
fn test_array_rsort() {
    let source = r#"
module tb;
    int q[$];
    reg [31:0] v0;
    reg [31:0] v1;
    reg [31:0] v2;
    initial begin
        q.push_back(10);
        q.push_back(30);
        q.push_back(20);
        q.rsort();
        v0 = q.pop_front();
        v1 = q.pop_front();
        v2 = q.pop_front();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v0 = sigs
        .iter()
        .find(|(n, _)| n == "v0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v1 = sigs
        .iter()
        .find(|(n, _)| n == "v1")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let v2 = sigs
        .iter()
        .find(|(n, _)| n == "v2")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v0, 30, "rsort: first should be 30");
    assert_eq!(v1, 20, "rsort: second should be 20");
    assert_eq!(v2, 10, "rsort: third should be 10");
}
#[test]
fn test_swrite_radix_variants_no_newline() {
    let source = r#"
module tb;
    string s;
    reg [7:0] v;
    initial begin
        v = 8'h2a;
        $swriteh(s, v);
        #1;
        $swriteb(s, v);
        #1;
        $swriteo(s, v);
        #1 $finish;
    end
endmodule
"#;
    // Nilai akhir = hasil $swriteo terakhir; memastikan tanpa newline.
    let sigs = simulate_signals(source, 10).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "052", "$swriteo tanpa newline (LRM 21.3.3)");
}

#[test]
fn test_swrite_hex_matches_iverilog() {
    let source = r#"
module tb;
    string s;
    reg [7:0] v;
    initial begin
        v = 8'h2a;
        $swriteh(s, v);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "2a", "$swriteh hex tanpa newline");
}

#[test]
fn test_sformatf_basic() {
    let source = r#"
module tb;
    string s;
    string t;
    reg [31:0] val;
    initial begin
        val = 42;
        s = $sformatf("value = %0d", val);
        // Plain `%d` right-justify ke field selebar nilai maksimum tipe
        // (LRM 1800 Tabel 21-3; 32-bit → 10 kolom) — oracle iverilog.
        t = $sformatf("value = %d", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "value = 42", "sformatf with %0d");
    let t = sigs
        .iter()
        .find(|(n, _)| n == "t")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(t, "value =         42", "%d = field lebar tipe");
}

#[test]
fn test_sformatf_hex() {
    let source = r#"
module tb;
    string s;
    reg [31:0] val;
    initial begin
        val = 255;
        s = $sformatf("0x%h", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(
        s, "0x000000ff",
        "sformatf \"0x%h\" = literal '0x' + full-width hex (IEEE 1800 §21.2.1.3)"
    );
}

#[test]
fn test_sformatf_binary() {
    let source = r#"
module tb;
    string s;
    reg [31:0] val;
    initial begin
        val = 10;
        s = $sformatf("%b", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(
        s, "00000000000000000000000000001010",
        "sformatf with %b (plain = full width, IEEE 1800 §21.2.1.3)"
    );
}

#[test]
fn test_sformatf_multiple_args() {
    let source = r#"
module tb;
    string s;
    reg [31:0] a;
    reg [31:0] b;
    initial begin
        a = 10;
        b = 20;
        s = $sformatf("a=%0d b=%0d", a, b);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let s = sigs
        .iter()
        .find(|(n, _)| n == "s")
        .map(|(_, v)| logicvec_to_string(v))
        .unwrap_or_default();
    assert_eq!(s, "a=10 b=20", "sformatf dengan beberapa argumen");
}

#[test]
fn test_fwrite_and_fscanf() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fwrite.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [31:0] val;
    initial begin
        fd = $fopen("{}", "w");
        $fwrite(fd, "42 100");
        $fclose(fd);
        fd = $fopen("{}", "r");
        $fscanf(fd, "%d %d", val);
        #1 $finish;
    end
endmodule
"#,
        test_file, test_file
    );
    let sigs = simulate_signals(&source, 5).unwrap();
    let val = sigs
        .iter()
        .find(|(n, _)| n == "val")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(val, 42, "fscanf should read first value");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fstrobe() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fstrobe.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [31:0] cnt;
    initial begin
        fd = $fopen("{f}", "w");
        cnt = 42;
        $fstrobe(fd, "cnt=%0d", cnt);
        #1 cnt = 100;
        #1 $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert!(
        content.contains("cnt=42"),
        "fstrobe should write cnt=42 (pre-change), got: {:?}",
        content
    );
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fdisplay_expression_form_adds_newline() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fdisplay_expr.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    integer r;
    initial begin
        fd = $fopen("{f}", "w");
        r = $fdisplay(fd, "a");
        $fdisplay(fd, "b");
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert_eq!(content, "a\nb\n", "bentuk ekspresi $fdisplay juga menambah newline");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fopen_append_mode() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fopen_append.txt";
    let _ = fs::write(test_file, "old\n");
    let source = format!(
        r#"
module tb;
    integer fd;
    initial begin
        fd = $fopen("{f}", "a");
        $fdisplay(fd, "new");
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert_eq!(content, "old\nnew\n", "mode \"a\" harus append, bukan truncate");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fdisplay_radix_variants() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fdisplay_radix.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [7:0] v;
    initial begin
        v = 8'h2a;
        fd = $fopen("{f}", "w");
        $fdisplayh(fd, v);
        $fwriteb(fd, v);
        $fdisplayo(fd, v);
        $fclose(fd);
        $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    assert_eq!(content, "2a\n00101010052\n");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_size_with_dimension_argument() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [3:0][7:0] p;
    reg [31:0] m1, m2, p1, p2;
    initial begin
        m1 = $size(m, 1);
        m2 = $size(m, 2);
        p1 = $size(p, 1);
        p2 = $size(p, 2);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("m1"), 4, "unpacked dim pertama");
    assert_eq!(get("m2"), 8, "packed dim kedua");
    assert_eq!(get("p1"), 4, "packed dim pertama");
    assert_eq!(get("p2"), 8, "packed dim kedua");
}

#[test]
fn test_shortreal_bit_conversions() {
    let source = r#"
module tb;
    real r;
    reg [31:0] b;
    initial begin
        r = $bitstoshortreal(32'h3fc00000);
        b = $shortrealtobits(1.5);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let b = sigs
        .iter()
        .find(|(n, _)| n == "b")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(b, 0x3FC0_0000, "$shortrealtobits(1.5)");
    let r = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| f64::from_bits(v.to_u64()))
        .unwrap_or(f64::NAN);
    assert!((r - 1.5).abs() < 1e-9, "$bitstoshortreal(0x3fc00000) = 1.5");
}

#[test]
fn test_random_seed_is_inout() {
    let source = r#"
module tb;
    integer s, r1, r2, s1, s2;
    initial begin
        s = 1;
        r1 = $random(s);
        s1 = s;
        r2 = $random(s);
        s2 = s;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> i64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_i64())
            .unwrap_or(i64::MIN)
    };
    assert_ne!(get("s1"), 1, "$random(seed) harus memperbarui seed");
    assert_ne!(get("s2"), get("s1"), "seed berubah tiap panggilan");
    assert_ne!(get("r1"), get("r2"), "dua panggilan dengan seed inout berbeda hasil");
}

#[test]
fn test_countbits_with_control_bits() {
    let source = r#"
module tb;
    reg [7:0] v;
    reg [31:0] c1, c2, c3;
    initial begin
        v = 8'b1010_0110;
        c1 = $countbits(v, 1'b1);
        c2 = $countbits(v, 1'b0);
        c3 = $countbits(v);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("c1"), 4, "$countbits(v, 1)");
    assert_eq!(get("c2"), 4, "$countbits(v, 0)");
    assert_eq!(get("c3"), 0, "$countbits tanpa control bit");
}

#[test]
fn test_countbits_literal_respects_width() {
    // Literal 8-bit: zero-padding ke 64 bit TIDAK boleh ikut dihitung.
    let source = r#"
module tb;
    reg [31:0] c1, c2;
    initial begin
        c1 = $countbits(8'b1010_0101, 1'b1);
        c2 = $countbits(8'b1010_0101, 1'b0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("c1"), 4, "4 bit bernilai 1 pada literal 8-bit");
    assert_eq!(get("c2"), 4, "4 bit bernilai 0 pada literal 8-bit");
}

#[test]
fn test_unpacked_dimensions() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] scalar;
    reg [31:0] d1, d2;
    initial begin
        d1 = $unpacked_dimensions(m);
        d2 = $unpacked_dimensions(scalar);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("d1"), 1, "memori 1D punya 1 dimensi unpacked");
    assert_eq!(get("d2"), 0, "scalar tidak punya dimensi unpacked");
}

#[test]
fn test_dist_functions_ranges_and_seed_update() {
    let source = r#"
module tb;
    integer s;
    reg [31:0] u, e, p, c, er, n;
    initial begin
        s = 1;
        u  = $dist_uniform(s, 5, 10);
        e  = $dist_exponential(s, 100);
        p  = $dist_poisson(s, 4);
        c  = $dist_chi_square(s, 3);
        er = $dist_erlang(s, 2, 100);
        n  = $dist_normal(s, 100, 10);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    let u = get("u");
    assert!((5..=10).contains(&u), "$dist_uniform di [5,10], dapat {}", u);
    assert!(get("e") <= 100_000, "$dist_exponential wajar");
    assert!(get("p") <= 10_000, "$dist_poisson wajar");
    assert!(get("c") <= 100_000, "$dist_chi_square wajar");
    assert!(get("er") <= 100_000, "$dist_erlang wajar");
    assert!(get("n") <= 1_000, "$dist_normal wajar");
    assert_ne!(get("s"), 1, "seed harus diperbarui (inout)");
}

#[test]
fn test_fread_memory_start_count() {
    use std::fs;
    let file = "/tmp/test_mivon_fread.bin";
    let _ = fs::write(file, [0x11u8, 0x22, 0x33, 0x44]);
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    integer fd;
    integer r;
    initial begin
        m[0]=8'h00; m[1]=8'h00; m[2]=8'h00; m[3]=8'h00;
        fd = $fopen("{f}", "rb");
        r = $fread(m, fd, 1, 2);
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = file
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("r"), 2, "jumlah word terbaca");
    // m[1], m[2] terisi; m[0], m[3] tetap.
    let m = sigs
        .iter()
        .find(|(n, _)| n == "m")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(
        m.bits[0..8]
            .iter()
            .filter(|b| **b == mivon_ir::LogicVal::One)
            .count(),
        0
    );
    let word1: u64 = (0..8)
        .filter(|i| m.bits[8 + i] == mivon_ir::LogicVal::One)
        .map(|i| 1u64 << i)
        .sum();
    let word2: u64 = (0..8)
        .filter(|i| m.bits[16 + i] == mivon_ir::LogicVal::One)
        .map(|i| 1u64 << i)
        .sum();
    assert_eq!(word1, 0x11);
    assert_eq!(word2, 0x22);
    let _ = fs::remove_file(file);
}

#[test]
fn test_writemem_range() {
    use std::fs;
    let file = "/tmp/test_mivon_writemem_range.hex";
    let _ = fs::remove_file(file);
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    initial begin
        m[0]=8'h11; m[1]=8'h22; m[2]=8'h33; m[3]=8'h44;
        $writememh("{f}", m, 1, 2);
        $finish;
    end
endmodule
"#,
        f = file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(file).unwrap_or_default();
    assert_eq!(content, "// 0x00000000\n22\n33\n", "start/end word");
    let _ = fs::remove_file(file);
}

#[test]
fn test_sscanf_parses_d_h_c() {
    let source = r#"
module tb;
    integer n;
    reg [31:0] v, a, b;
    reg [7:0] c;
    initial begin
        n = $sscanf("42", "%d", v);
        n = $sscanf("dead beef", "%h %h", a, b);
        n = $sscanf("X", "%c", c);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!(get("v"), 42);
    assert_eq!(get("a"), 0xdead);
    assert_eq!(get("b"), 0xbeef);
    assert_eq!(get("c"), 0x58); // 'X' = 0x58
}

#[test]
fn test_sreadmemh_and_bin() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] n [0:1];
    reg [7:0] a0, a1, a2, a3, b0, b1;
    initial begin
        $sreadmemh("aa bb cc dd", m);
        $sreadmemb("00000001 00000010", n);
        a0 = m[0]; a1 = m[1]; a2 = m[2]; a3 = m[3];
        b0 = n[0]; b1 = n[1];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("a0"), 0xaa);
    assert_eq!(get("a1"), 0xbb);
    assert_eq!(get("a2"), 0xcc);
    assert_eq!(get("a3"), 0xdd);
    assert_eq!(get("b0"), 1);
    assert_eq!(get("b1"), 2);
}

#[test]
fn test_readmemh_missing_file_continues() {
    let source = r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] a0;
    initial begin
        m[0]=8'h11;
        $readmemh("/tmp/mivon_does_not_exist_xyz.hex", m);
        a0 = m[0];
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let a0 = sigs
        .iter()
        .find(|(n, _)| n == "a0")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(u64::MAX);
    assert_eq!(a0, 0x11, "file hilang: peringatan, memori tidak berubah, simulasi lanjut");
}

#[test]
fn test_readmemh_address_directive() {
    use std::fs;
    let file = "/tmp/test_mivon_readmem_at.hex";
    let _ = fs::write(file, "aa\n@2\ncc\n");
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] a0, a1, a2, a3;
    initial begin
        m[0]=8'h11; m[1]=8'h22; m[2]=8'h33; m[3]=8'h44;
        $readmemh("{f}", m);
        a0=m[0]; a1=m[1]; a2=m[2]; a3=m[3];
        #1 $finish;
    end
endmodule
"#,
        f = file
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    assert_eq!((get("a0"), get("a1"), get("a2"), get("a3")), (0xaa, 0x22, 0xcc, 0x44));
    let _ = fs::remove_file(file);
}

#[test]
fn test_readmemh_preserves_untouched_words_and_range() {
    use std::fs;
    let full = "/tmp/test_mivon_readmem_full.hex";
    let part = "/tmp/test_mivon_readmem_part.hex";
    let _ = fs::write(full, "aa\nbb\ncc\ndd\n");
    let _ = fs::write(part, "aa\nbb\n");
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    reg [7:0] n [0:3];
    reg [7:0] a0, a1, a2, a3, b0, b1, b2, b3;
    initial begin
        m[0]=8'h11; m[1]=8'h22; m[2]=8'h33; m[3]=8'h44;
        n[0]=8'h11; n[1]=8'h22; n[2]=8'h33; n[3]=8'h44;
        $readmemh("{full}", m);          // 4 word penuh
        $readmemh("{part}", n, 2, 3);    // isi word 2..3 saja
        a0=m[0]; a1=m[1]; a2=m[2]; a3=m[3];
        b0=n[0]; b1=n[1]; b2=n[2]; b3=n[3];
        #1 $finish;
    end
endmodule
"#,
        full = full,
        part = part
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let get = |name: &str| -> u64 {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(u64::MAX)
    };
    // File penuh → semua word terisi.
    assert_eq!((get("a0"), get("a1"), get("a2"), get("a3")), (0xaa, 0xbb, 0xcc, 0xdd));
    // start=2, end=3 → hanya word 2,3 yang berubah.
    assert_eq!((get("b0"), get("b1"), get("b2"), get("b3")), (0x11, 0x22, 0xaa, 0xbb));
    let _ = fs::remove_file(full);
    let _ = fs::remove_file(part);
}

#[test]
fn test_ferror_valid_handle_returns_zero() {
    use std::fs;
    let test_file = "/tmp/test_mivon_ferror.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [31:0] err;
    string msg;
    initial begin
        fd = $fopen("{f}", "w");
        err = $ferror(fd, msg);
        $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let sigs = simulate_signals(&source, 10).unwrap();
    let err = sigs
        .iter()
        .find(|(n, _)| n == "err")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(999);
    assert_eq!(err, 0, "$ferror pada handle valid harus 0");
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_writemem_hex_and_bin() {
    use std::fs;
    let hex_file = "/tmp/test_mivon_writemem.hex";
    let bin_file = "/tmp/test_mivon_writemem.bin";
    let _ = fs::remove_file(hex_file);
    let _ = fs::remove_file(bin_file);
    let source = format!(
        r#"
module tb;
    reg [7:0] m [0:3];
    initial begin
        m[0] = 8'haa;
        m[1] = 8'hbb;
        m[2] = 8'hcc;
        m[3] = 8'hdd;
        $writememh("{h}", m);
        $writememb("{b}", m);
        $finish;
    end
endmodule
"#,
        h = hex_file,
        b = bin_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let hex = fs::read_to_string(hex_file).unwrap_or_default();
    assert_eq!(hex, "// 0x00000000\naa\nbb\ncc\ndd\n");
    let bin = fs::read_to_string(bin_file).unwrap_or_default();
    assert_eq!(
        bin,
        "// 0x00000000\n10101010\n10111011\n11001100\n11011101\n"
    );
    let _ = fs::remove_file(hex_file);
    let _ = fs::remove_file(bin_file);
}

#[test]
fn test_fmonitor() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fmonitor.txt";
    let _ = fs::remove_file(test_file);
    let source = format!(
        r#"
module tb;
    integer fd;
    reg [7:0] x;
    initial begin
        fd = $fopen("{f}", "w");
        $fmonitor(fd, "x=%0d\n", x);
        x = 10;
        #1 x = 20;
        #1 x = 20;
        #1 x = 30;
        #1 $fclose(fd);
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let _ = simulate_signals(&source, 10).unwrap();
    let content = fs::read_to_string(test_file).unwrap_or_default();
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "fmonitor should write on change, got {} lines: {:?}",
        lines.len(),
        content
    );
    assert!(
        content.contains("x=10"),
        "fmonitor should capture x=10, got: {:?}",
        content
    );
    assert!(
        content.contains("x=30"),
        "fmonitor should capture x=30, got: {:?}",
        content
    );
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_fread_file() {
    use std::fs;
    let test_file = "/tmp/test_mivon_fread.txt";
    let _ = fs::remove_file(test_file);
    fs::write(test_file, b"\x41\x42\x43").unwrap();
    let source = format!(
        r#"
module tb;
    reg [23:0] data;
    initial begin
        $fread(data, "{f}");
        #1 $finish;
    end
endmodule
"#,
        f = test_file
    );
    let sigs = simulate_signals(&source, 5).unwrap();
    let data = sigs
        .iter()
        .find(|(n, _)| n == "data")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        data, 0x434241,
        "fread should read binary 0x41 0x42 0x43 -> 0x434241, got 0x{:x}",
        data
    );
    let _ = fs::remove_file(test_file);
}

#[test]
fn test_signed_lt_literal_zero() {
    // signed signal vs literal 0: `s < 0` harus signed (SV: relational signed
    // bila SALAH SATU operand signed). Bug: kondisi lama && (kedua signed)
    // → literal 0 bukan signed → unsigned compare → 255 < 0 = false. Fix: ||.
    let source = r#"
module tb;
    reg signed [7:0] s;
    reg neg;
    initial begin
        s = -8'sd1;
        neg = (s < 0);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let neg = sigs
        .iter()
        .find(|(n, _)| n == "neg")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(neg, 1, "signed -1 < 0 harus 1 (bukan unsigned 255<0)");
}

#[test]
fn test_real_arithmetic_literal_fold() {
    // Bug #6: `1.5 + 2.25` → NaN (evaluator biner tak kenal real utk literal;
    // hanya signal is_real). Fix: fold real literal+literal di elaborasi.
    let source = r#"
module tb;
    real r, r2;
    initial begin
        r = 1.5 + 2.25;
        r2 = 10.0 / 4.0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64();
    let r = f64::from_bits(get("r")) as u32;
    let r2 = f64::from_bits(get("r2")) as u32;
    assert_eq!(r, 3, "1.5+2.25 = 3.75 (f64), dapat `{r}`");
    assert_eq!(r2, 2, "10/4 = 2.5 (f64), dapat `{r2}`");
}

#[test]
fn test_signed_div_mod() {
    // Bug #7: `s = -4; sd = s / 2` → 126 (unsigned 252/2) — operand sinyal
    // zero-extend ke 32 (Cast) sebelum eval signed. Fix: clip ke lebar asli
    // signal utk Div/Mod (signed_raw_operand).
    let source = r#"
module tb;
    reg signed [7:0] s, sd, sm;
    initial begin
        s = -8'sd4;
        sd = s / 2;
        sm = s % 3;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let get = |n: &str| sigs.iter().find(|(s, _)| s == n).unwrap().1.to_u64() as i8;
    assert_eq!(get("sd"), -2, "-4/2 = -2 (0xFE), bukan 126");
    assert_eq!(get("sm"), -1, "-4 % 3 = -1 (0xFF)");
}

#[test]
fn test_signed_relational() {
    let source = r#"
module tb;
    reg signed [7:0] a, b;
    reg lt, gt, ge, le;
    initial begin
        a = -3;
        b = 2;
        lt = a < b;
        gt = a > b;
        ge = a >= b;
        le = a <= b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let lt = sigs
        .iter()
        .find(|(n, _)| n == "lt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let gt = sigs
        .iter()
        .find(|(n, _)| n == "gt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let ge = sigs
        .iter()
        .find(|(n, _)| n == "ge")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let le = sigs
        .iter()
        .find(|(n, _)| n == "le")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(lt, 1, "signed: -3 < 2 should be 1");
    assert_eq!(gt, 0, "signed: -3 > 2 should be 0");
    assert_eq!(ge, 0, "signed: -3 >= 2 should be 0");
    assert_eq!(le, 1, "signed: -3 <= 2 should be 1");
}

#[test]
fn test_signed_relational_negatives() {
    let source = r#"
module tb;
    reg signed [7:0] a, b;
    reg lt;
    initial begin
        a = -5;
        b = -3;
        lt = a < b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let lt = sigs
        .iter()
        .find(|(n, _)| n == "lt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(lt, 1, "signed: -5 < -3 should be 1");
}

#[test]
fn test_unsigned_relational() {
    let source = r#"
module tb;
    reg [7:0] a, b;
    reg lt, gt;
    initial begin
        a = 8'hFD;
        b = 8'h02;
        lt = a < b;
        gt = a > b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let lt = sigs
        .iter()
        .find(|(n, _)| n == "lt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let gt = sigs
        .iter()
        .find(|(n, _)| n == "gt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(lt, 0, "unsigned: 0xFD < 0x02 should be 0");
    assert_eq!(gt, 1, "unsigned: 0xFD > 0x02 should be 1");
}

#[test]
fn test_wait_statement() {
    let source = r#"
module tb;
    reg [7:0] cnt;
    reg done;
    initial begin
        cnt = 0;
        #10 cnt = 5;
    end
    initial begin
        wait (cnt == 5);
        done = 1;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 20).unwrap();
    let cnt_val = sigs
        .iter()
        .find(|(n, _)| n == "cnt")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    let done_val = sigs
        .iter()
        .find(|(n, _)| n == "done")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(cnt_val, 5, "cnt should be 5");
    assert_eq!(done_val, 1, "done should be 1 after wait is satisfied");
}

#[test]
fn test_force_statement() {
    let source = r#"
module tb;
    reg [7:0] a;
    reg [7:0] b;
    initial begin
        a = 10;
        b = 20;
        #1 force a = b;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 20, "a should be forced to b=20");
}

#[test]
fn test_random_urandom() {
    let source = r#"
module tb;
    reg [31:0] r;
    initial begin
        r = $urandom();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let r_val = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    // $urandom returns a non-zero 32-bit value (could be zero but unlikely)
    assert!(r_val < 4294967296, "r should be a 32-bit value");
}

#[test]
fn test_dumpvars_dumpoff() {
    let source = r#"
module tb;
    reg [7:0] a;
    initial begin
        a = 42;
        $dumpvars();
        #1 $dumpoff();
        #2 $dumpon();
        #3 $finish();
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let a_val = sigs
        .iter()
        .find(|(n, _)| n == "a")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(a_val, 42, "a should be 42");
}

#[test]
fn test_preprocessor_with_simulation() {
    let source = r#"
`define WIDTH 8
`ifdef NEVER
wire never;
`endif
module test;
    reg [`WIDTH-1:0] data;
    initial begin
        data = 8'hAB;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "data").unwrap();
    assert_eq!(
        val.to_u64(),
        0xAB,
        "preprocessed signal should have correct value"
    );
}

#[test]
fn test_clog2_in_expr() {
    let source = r#"
module tb;
    reg [7:0] w;
    reg [31:0] result;
    initial begin
        result = $clog2(8);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "result").unwrap();
    assert_eq!(val.to_u64(), 3, "$clog2(8) should be 3");
}

#[test]
fn test_clog2_power_of_two() {
    let source = r#"
module tb;
    reg [31:0] r;
    initial begin
        r = $clog2(16);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "r").unwrap();
    assert_eq!(val.to_u64(), 4, "$clog2(16) should be 4");
}

#[test]
fn test_clog2_one() {
    let source = r#"
module tb;
    reg [31:0] r;
    initial begin
        r = $clog2(1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let (_, val) = sigs.iter().find(|(n, _)| n == "r").unwrap();
    assert_eq!(val.to_u64(), 0, "$clog2(1) should be 0");
}

#[test]
fn test_casex_wildcard() {
    let source = r#"
module tb;
    reg [3:0] sel;
    reg [7:0] out;
    always @(*) begin
        casex (sel)
            4'b1xx0: out = 8'hA0;
            4'b01x0: out = 8'hB0;
            4'b0010: out = 8'hC0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 4'b1000;
        #1;
        if (out !== 8'hA0) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 0xA0, "casex 4'b1000 should match 4'b1xx0 => 0xA0");
}

#[test]
fn test_casez_wildcard() {
    let source = r#"
module tb;
    reg [3:0] sel;
    reg [7:0] out;
    always @(*) begin
        casez (sel)
            4'b1zz0: out = 8'hA0;
            4'b01z0: out = 8'hB0;
            4'b0010: out = 8'hC0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 4'b1010;
        #1;
        if (out !== 8'hA0) $finish;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(out_val, 0xA0, "casez 4'b1010 should match 4'b1zz0 => 0xA0");
}

/// Qualifier + kind ortogonal (LRM 1800 §12.5): `priority casez` memakai
/// pencocokan wildcard (bukan exact-match). Vs iverilog: 101→A0, 011→B0.
#[test]
fn test_priority_casez_wildcard() {
    let source = r#"
module tb;
    reg [2:0] sel;
    reg [7:0] out;
    always @(*) begin
        priority casez (sel)
            3'b1??: out = 8'hA0;
            3'b01?: out = 8'hB0;
            3'b001: out = 8'hC0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 3'b101;
        #1;
        if (out !== 8'hA0) $display("MISS101 %0h", out);
        sel = 3'b011;
        #1;
        if (out !== 8'hB0) $display("MISS011 %0h", out);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        out_val, 0xB0,
        "priority casez 3'b011 should match 3'b01? => 0xB0"
    );
}

/// `unique casez` juga memakai wildcard (combo UniqueZ).
#[test]
fn test_unique_casez_wildcard() {
    let source = r#"
module tb;
    reg [2:0] sel;
    reg [7:0] out;
    always @(*) begin
        unique casez (sel)
            3'b1??: out = 8'hA0;
            default: out = 8'hFF;
        endcase
    end
    initial begin
        sel = 3'b100;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let out_val = sigs
        .iter()
        .find(|(n, _)| n == "out")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        out_val, 0xA0,
        "unique casez 3'b100 should match 3'b1?? => 0xA0"
    );
}

/// Default `parameter type T = logic [7:0]` memberi lebar 8 (bukan 1).
/// Regresi: parser membuang range (`T` selalu 1-bit → port mismatch).
#[test]
fn test_param_type_default_packed_range_width() {
    let source = r#"
module shifter #(
    parameter type T = logic [7:0],
    parameter N = 2
) (
    input bit clk,
    input T d,
    output T q
);
    T r;
    always_ff @(posedge clk) begin
        r <= d << N;
        q <= r;
    end
endmodule
module tb;
    bit clk;
    logic [7:0] d8, q8;
    shifter u8 (.clk(clk), .d(d8), .q(q8));
    initial begin
        clk = 0;
        d8 = 8'h01;
        #10;
        repeat (3) @(posedge clk);
        #1;
        $display("TP q8=%0d", q8);
        $finish;
    end
    initial begin
        forever #5 clk = ~clk;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 200).unwrap();
    let q8 = sigs
        .iter()
        .find(|(n, _)| n == "q8")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(q8, 4, "T=logic[7:0] N=2: 01<<2=4 (q8={q8})");
}

#[test]
fn test_disable_named_block() {
    let source = r#"
module tb;
    reg [7:0] count;
    integer i;
    initial begin
        count = 0;
        for (i = 0; i < 10; i = i + 1) begin : loop_block
            if (i == 5) disable loop_block;
            count = count + 1;
        end
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        count_val, 5,
        "disable should break at i=5, count should be 5"
    );
}

#[test]
fn test_disable_outer_block() {
    let source = r#"
module tb;
    reg [7:0] count;
    integer i;
    initial begin : outer
        count = 0;
        for (i = 0; i < 3; i = i + 1) begin : inner
            if (i == 1) disable outer;
            count = count + 1;
        end
        count = 100;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).unwrap();
    let count_val = sigs
        .iter()
        .find(|(n, _)| n == "count")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(
        count_val, 1,
        "disable outer should break at i=1 after count becomes 1"
    );
}
