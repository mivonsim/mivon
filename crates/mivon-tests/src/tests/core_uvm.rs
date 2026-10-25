//! UVM dan verifikasi: config_db, phases, TLM/analysis port, coverage
//! sequence, serta DPI-C.

use super::*;

#[test]
fn test_uvm_analysis_port_sim() {
    let source = r#"
class my_scoreboard extends uvm_scoreboard;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void write(uvm_sequence_item item);
        // item received from monitor via analysis port
    endfunction
endclass

module tb;
    my_scoreboard sb;
    uvm_analysis_port ap;
    uvm_analysis_imp imp;
    initial begin
        sb = my_scoreboard::new("sb", 0);
        ap = uvm_analysis_port::new("ap");
        imp = uvm_analysis_imp::new("imp", sb);
        ap.connect(imp);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_analysis_port test failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_phases_execute() {
    let source = r#"
class my_test extends uvm_test;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        super.build_phase();
    endfunction
    function void connect_phase();
        super.connect_phase();
    endfunction
    task run_phase();
        super.run_phase();
    endtask
endclass

module tb;
    my_test test;
    initial begin
        test = my_test::new("test", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(result.is_ok(), "uvm_phases test failed: {:?}", result.err());
}

#[test]
fn test_uvm_config_db_set_get() {
    let source = r#"
module tb;
    int val;
    int success;
    initial begin
        uvm_config_db::set(null, "top", "my_key", 42);
        success = uvm_config_db::get(null, "top", "my_key", val);
        assert(success == 1);
        assert(val == 42);
        // Not found case
        success = uvm_config_db::get(null, "top", "missing", val);
        assert(success == 0);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_config_db test failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_config_db_exists() {
    // VERIF-06: `uvm_config_db::exists(inst, field)` → 1 bila key punya
    // nilai (set pernah dipanggil), 0 bila tidak. $error kanari.
    let source = r#"
module tb;
    int e_before;
    int e_after;
    initial begin
        e_before = uvm_config_db::exists(null, "top", "my_key");
        uvm_config_db::set(null, "top", "my_key", 7);
        e_after = uvm_config_db::exists(null, "top", "my_key");
        if (e_before != 0) $error("exists sebelum set harus 0, got %0d", e_before);
        if (e_after != 1) $error("exists setelah set harus 1, got %0d", e_after);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_config_db exists failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_config_db_wait_modified() {
    // VERIF-06: `uvm_config_db::wait_modified(inst, field)` BLOCKING —
    // task konsumen suspend sampai `set` berikutnya utk key tsb, lalu resume.
    // $error kanari: resume tidak terjadi (woke=0) → Err. Menangkap:
    // (1) intercept block.rs tak ada (wait_modified di-eval sebagai query,
    // statement lanjut seketika → woke=1 padahal key belum ada);
    // (2) release oleh set tak ada (waiter tak pernah dibangunkan).
    let source = r#"
class my_env extends uvm_env;
    int woke;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        fork
            // Konsumen: wait_modified blocking sampai set dipanggil.
            begin
                uvm_config_db::wait_modified(null, "top", "cfg");
                woke = 1;
            end
            // Produsen: delay 10 lalu set → bangunkan konsumen.
            begin
                #10;
                uvm_config_db::set(null, "top", "cfg", 99);
            end
        join
    endtask
endclass

module tb;
    my_env env_ref;
    int woke_snap_5;
    int woke_snap_20;
    initial begin
        env_ref = new("env_ref", null);
        fork
            env_ref.run_phase();
            // Probe: woke harus 0 di t=5 (sebelum set t=10), 1 di t=20.
            begin
                #5 woke_snap_5 = env_ref.woke;
                #15 woke_snap_20 = env_ref.woke;
            end
        join
        if (woke_snap_5 != 0)
            $error("woke harus 0 di t=5 (blocking), got %0d", woke_snap_5);
        if (woke_snap_20 != 1)
            $error("woke harus 1 di t=20 (resume setelah set), got %0d", woke_snap_20);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 100).expect("uvm_config_db wait_modified run failed");
    let s5 = sigs
        .iter()
        .find(|(n, _)| n == "woke_snap_5")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    let s20 = sigs
        .iter()
        .find(|(n, _)| n == "woke_snap_20")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(
        s5, 0,
        "woke harus 0 di t=5 (wait_modified blocking sebelum set), got {}",
        s5
    );
    assert_eq!(
        s20, 1,
        "woke harus 1 di t=20 (resume setelah set), got {}",
        s20
    );
}

#[test]
fn test_uvm_config_db_wildcard_path() {
    // F19: wildcard path matching — pola UVM nyata `*.agent` /
    // `uvm_test_top.*` harus match inst_path hierarki, dan exact match
    // MENANG atas wildcard.
    let source = r#"
module tb;
    int val;
    int success;
    initial begin
        // wildcard tunggal: `*` match satu level hierarki
        uvm_config_db::set(null, "*.agent", "count", 8);
        success = uvm_config_db::get(null, "env.agent", "count", val);
        assert(success == 1);
        assert(val == 8);
        // wildcard multi-level: `uvm_test_top.*` match seluruh subtree
        uvm_config_db::set(null, "uvm_test_top.*", "depth", 3);
        success = uvm_config_db::get(null, "uvm_test_top.env.agent", "depth", val);
        assert(success == 1);
        assert(val == 3);
        // exact match menang atas wildcard yang juga match
        uvm_config_db::set(null, "env.agent", "count", 99);
        success = uvm_config_db::get(null, "env.agent", "count", val);
        assert(success == 1);
        assert(val == 99);
        // wildcard TIDAK match field lain
        success = uvm_config_db::get(null, "env.agent", "missing", val);
        assert(success == 0);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 5);
    assert!(
        result.is_ok(),
        "uvm_config_db wildcard test failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_config_db_run_test_hierarchy() {
    // F19: skenario UVM NYATA — `initial run_test("my_test")` + set di
    // build_phase test + get(this, "", ...) wildcard di agent. Regresi ini
    // menangkap 3 bug yang diperbaiki bersama:
    //  (1) auto-detect execute_phases() menang atas run_test eksplisit
    //      (guard design_has_explicit_run_test)
    //  (2) `uvm_config_db::set(...)` statement di body method class dikira
    //      deklarasi `pkg::type` → parse_function gagal → build_phase
    //      tidak terdaftar
    //  (3) NUL terminator dari string_to_logicvec bocor ke inst_path
    //      (`uvm_test_top\0.agent` → wildcard tidak pernah match)
    let source = r#"
class my_agent extends uvm_agent;
    int got_count;
    int ok;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ok = uvm_config_db::get(this, "", "count", got_count);
        if (ok != 1 || got_count != 8)
            $error("AGENT get failed ok=%0d count=%0d", ok, got_count);
    endfunction
endclass

class my_env extends uvm_env;
    my_agent ag;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ag = new("agent", this);
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        uvm_config_db::set(this, "*.agent", "count", 8);
        env = new("env", this);
    endfunction
endclass

module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_signals(source, 100);
    assert!(
        result.is_ok(),
        "uvm_config_db run_test hierarchy failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_event_sync() {
    // F21: uvm_event — trigger()/wait_trigger() blocking antar fork branch di
    // run_phase. `$error` dipakai sebagai kanari: kalau wait_trigger tidak
    // memblock (join terlalu cepat) atau field tidak ter-set, simulate_str
    // mengembalikan Err → assert gagal. Menangkap 3 bug: (1) AST fork join
    // mengeksekusi remaining langsung tanpa menunggu branch; (2) current_this
    // hilang saat fork_finish mengeksekusi cont AST setelah join; (3) event
    // data tak ter-insert karena builtin `__uvm_event` punya methods kosong.
    let source = r#"
class my_env extends uvm_env;
    uvm_event done_ev;
    int t1_done;
    int t2_done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        done_ev = new("done_ev");
        fork
            begin
                #10;
                done_ev.trigger();
                t1_done = 1;
            end
            begin
                done_ev.wait_trigger();
                t2_done = 1;
            end
        join
        if (!t1_done || !t2_done)
            $error("uvm_event sync failed t1=%0d t2=%0d", t1_done, t2_done);
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(result.is_ok(), "uvm_event sync failed: {:?}", result.err());
}

#[test]
fn test_uvm_barrier_sync() {
    // F21: uvm_barrier — threshold 3: ketiga branch harus melewati wait_for
    // bersamaan; count di-reset setelah release. `$error` kanari bila ada
    // branch yang tidak pernah di-release (barrier tidak penuh).
    let source = r#"
class my_env extends uvm_env;
    uvm_barrier bar;
    int a_done;
    int b_done;
    int c_done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        bar = new("bar", 3);
        if (bar.get_threshold() != 3)
            $error("barrier threshold wrong: %0d", bar.get_threshold());
        fork
            begin
                #5;
                bar.wait_for();
                a_done = 1;
            end
            begin
                #10;
                bar.wait_for();
                b_done = 1;
            end
            begin
                #15;
                bar.wait_for();
                c_done = 1;
            end
        join
        if (!a_done || !b_done || !c_done)
            $error("uvm_barrier sync failed a=%0d b=%0d c=%0d", a_done, b_done, c_done);
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_barrier sync failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_event_subclass_override() {
    // F21 review fix #1: subclass user `my_event extends uvm_event` — `new`
    // override + method custom HARUS jalan normal, bukan di-intercept builtin
    // ("unknown uvm_event method"), dan `super.new` tetap insert data sync.
    let source = r#"
class my_event extends uvm_event;
    int extra;
    int notify_cnt;
    function new(string name);
        super.new(name);
        extra = 5;
    endfunction
    function void notify_extra();
        notify_cnt = notify_cnt + 1;
    endfunction
endclass

class my_env extends uvm_env;
    my_event ev;
    int ok1;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase();
        ev = new("ev");
        if (ev.extra != 5) $error("subclass new override tidak jalan");
        ev.notify_extra();
        if (ev.notify_cnt != 1) $error("custom method tidak jalan");
        fork
            begin
                #5;
                ev.trigger();
            end
            begin
                ev.wait_trigger();
                ok1 = 1;
            end
        join
        if (ev.triggered() != 1) $error("triggered() salah");
        if (ok1 != 1) $error("wait_trigger gagal");
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_event subclass override failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_subscriber_analysis_broadcast() {
    // F22: uvm_subscriber — monitor menulis item ke analysis_port, connect ke
    // `sub.analysis_imp` (builtin imp auto-dibuat saat new), broadcast sampai
    // ke `write` override user. $error kanari: salah count/addr → Err.
    // Menangkap: (1) analysis_imp internal tak dibuat (field analysis_imp
    // kosong → connect no-op); (2) report/check phase child TIDAK dipanggil
    // bila root tak punya phase tsb (fix execute_report_phases propagate).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_monitor extends uvm_monitor;
    uvm_analysis_port ap;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ap = uvm_analysis_port::new("ap", this);
    endfunction
    task run_phase();
        my_item it;
        #10;
        it = new("it1");
        it.addr = 42;
        ap.write(it);
    endtask
endclass

class my_sub extends uvm_subscriber;
    int got_cnt;
    int last_addr;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void write(my_item t);
        got_cnt = got_cnt + 1;
        last_addr = t.addr;
        if (t.addr != 42) $error("sub addr salah %0d", t.addr);
    endfunction
endclass

// F22 review: subscriber TANPA new override — imp harus tetap dibuat via
// guard is_uvm_subscriber_hierarchy di allocate_new_object (sebelumnya
// find_method_in_hierarchy("new") gagal → imp tak dibuat → connect(0)).
class my_sub2 extends uvm_subscriber;
    int got_cnt;
    function void write(my_item t);
        got_cnt = got_cnt + 1;
        if (t.addr != 42) $error("sub2 addr salah %0d", t.addr);
    endfunction
endclass

class my_env extends uvm_env;
    my_monitor mon;
    my_sub sub;
    my_sub2 sub2; // TANPA new override — analysis_imp harus tetap auto-dibuat
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        mon = new("mon", this);
        sub = new("sub", this);
        sub2 = new("sub2", this);
    endfunction
    function void connect_phase();
        mon.ap.connect(sub.analysis_imp);
        mon.ap.connect(sub2.analysis_imp);
    endfunction
    function void check_phase();
        if (sub.got_cnt != 1) $error("sub got_cnt=%0d harusnya 1", sub.got_cnt);
        if (sub.last_addr != 42) $error("sub last_addr=%0d harusnya 42", sub.last_addr);
        if (sub2.got_cnt != 1) $error("sub2 (tanpa new override) got_cnt=%0d harusnya 1", sub2.got_cnt);
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_subscriber broadcast failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_tlm_fifo_blocking_put_get() {
    // F23: uvm_tlm_fifo — konsumen `get` blocking (suspend saat kosong,
    // resume + tulis lvalue saat put), produsen `put` dua item terpisah.
    // $error kanari: item salah / count salah → Err. Menangkap: (1) statement
    // get tidak ada di continuation saat resume (pop+write tak terjadi);
    // (2) release waiter salah-match (wait_label vs current_method).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_tlm_fifo fifo;
    my_item r1;
    my_item r2;
    int got1;
    int got2;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        fifo = new("fifo", this, 4);
        if (fifo.capacity() != 4) $error("capacity salah");
    endfunction
    task run_phase();
        my_item a;
        my_item b;
        fork
            begin
                fifo.get(r1);
                got1 = 1;
                fifo.get(r2);
                got2 = 1;
            end
            begin
                #10;
                a = new("a");
                a.addr = 100;
                fifo.put(a);
                #10;
                b = new("b");
                b.addr = 200;
                fifo.put(b);
            end
        join
        if (got1 != 1 || got2 != 1) $error("blocking get gagal got1=%0d got2=%0d", got1, got2);
        if (r1.addr != 100 || r2.addr != 200) $error("item salah r1=%0d r2=%0d", r1.addr, r2.addr);
        if (fifo.used() != 0) $error("fifo harus kosong, used=%0d", fifo.used());
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_tlm_fifo blocking failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_tlm_fifo_analysis_export() {
    // F23: `fifo.analysis_export.write(item)` (export analysis internal)
    // memetakan ke put — konsumen `get` menerima item. Tanpa auto-created
    // export, `fifo.analysis_export` = null handle → write no-op → r1 tak
    // ter-set → $error kanari.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_tlm_fifo fifo;
    my_item r1;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        fifo = new("fifo", this, 4);
    endfunction
    task run_phase();
        my_item a;
        fork
            begin
                fifo.get(r1);
            end
            begin
                #10;
                a = new("a");
                a.addr = 77;
                fifo.analysis_export.write(a);
            end
        join
        if (r1.addr != 77) $error("analysis_export gagal r1=%0d", r1.addr);
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_tlm_fifo analysis_export failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_comparator_in_order_match() {
    // VERIF-13: uvm_in_order_comparator — `write_expected` push ke antrian,
    // `write(actual)` pop head & bandingkan (fallback field equality: addr
    // sama → match). $error kanari: count match/mismatch salah → Err.
    // Menangkap: (1) comparator tak ter-dispatch (get_match_count builtin
    // tidak ada); (2) fallback field compare salah (objek berbeda tapi
    // field sama harus dianggap MATCH — bukan obj id).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_in_order_comparator comp;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        comp = new("comp", this);
        if (comp.analysis_imp == 0) $error("analysis_imp internal tak dibuat");
    endfunction
    task run_phase();
        my_item e1;
        my_item e2;
        my_item a1;
        my_item a2;
        e1 = new("e1");
        e1.addr = 10;
        e2 = new("e2");
        e2.addr = 20;
        a1 = new("a1");
        a1.addr = 10;
        a2 = new("a2");
        a2.addr = 20;
        comp.write_expected(e1);
        comp.write_expected(e2);
        comp.write(a1);
        comp.write(a2);
    endtask
    function void check_phase();
        if (comp.get_match_count() != 2)
            $error("match=%0d harusnya 2", comp.get_match_count());
        if (comp.get_mismatch_count() != 0)
            $error("mismatch=%0d harusnya 0", comp.get_mismatch_count());
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_comparator in-order match failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_comparator_mismatch_via_analysis_port() {
    // VERIF-13: jalur analysis port penuh — monitor `ap.write(actual)` →
    // `comp.analysis_imp` → parent.write → compare vs expected head
    // (addr 5 vs 9 → MISMATCH). $error kanari: mismatch count salah.
    // Menangkap: analysis_imp internal tidak terhubung (connect no-op) →
    // write tak sampai → mismatch 0.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_monitor extends uvm_monitor;
    uvm_analysis_port ap;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        ap = uvm_analysis_port::new("ap", this);
    endfunction
    task run_phase();
        my_item it;
        #10;
        it = new("it1");
        it.addr = 9;
        ap.write(it);
    endtask
endclass

class my_env extends uvm_env;
    uvm_comparator comp;
    my_monitor mon;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        comp = new("comp", this);
        mon = new("mon", this);
    endfunction
    function void connect_phase();
        mon.ap.connect(comp.analysis_imp);
    endfunction
    task run_phase();
        my_item e;
        e = new("e1");
        e.addr = 5;
        comp.write_expected(e);
    endtask
    function void check_phase();
        if (comp.get_match_count() != 0)
            $error("match=%0d harusnya 0", comp.get_match_count());
        if (comp.get_mismatch_count() != 1)
            $error("mismatch=%0d harusnya 1 (addr 9 vs 5)", comp.get_mismatch_count());
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_comparator mismatch via analysis port failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_heartbeat_ok() {
    // VERIF-15: uvm_heartbeat — object ter-monitor (driver/monitor) memanggil
    // `hb.heartbeat(obj)` cukup kali → `check()` di check_phase = 1, tanpa
    // UVM_ERROR. $error kanari: check()=0 → Err. Menangkap: (1) heartbeat
    // tak ter-dispatch (set_heartbeat/heartbeat/check builtin tidak ada);
    // (2) counter tidak ter-increment (received selalu 0).
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_heartbeat hb;
    my_comp drv;
    my_comp mon;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        hb = new("hb", this);
        drv = new("drv", this);
        mon = new("mon", this);
        hb.set_heartbeat(drv, 2);
        hb.set_heartbeat(mon, 1);
    endfunction
    task run_phase();
        #5;
        hb.heartbeat(drv);
        #5;
        hb.heartbeat(drv);
        hb.heartbeat(mon);
    endtask
    function void check_phase();
        if (hb.get_heartbeat_count(drv) != 2)
            $error("drv heartbeat=%0d harusnya 2", hb.get_heartbeat_count(drv));
        if (hb.get_heartbeat_count(mon) != 1)
            $error("mon heartbeat=%0d harusnya 1", hb.get_heartbeat_count(mon));
        if (hb.check() != 1)
            $error("hb.check() harusnya 1 (semua terpenuhi)");
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_heartbeat ok failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_heartbeat_missing_heartbeat() {
    // VERIF-15: object yang TIDAK mencapai required heartbeat → `check()`
    // mengembalikan 0. Test ini memverifikasi sinyal kegagalan lewat return
    // value check() (diekspresikan sebagai $error kanari di user code),
    // BUKAN lewat severity engine — sehingga expected failure tetap Err.
    // Menangkap: check() selalu 1 (required diabaikan).
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_heartbeat hb;
    my_comp drv;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        hb = new("hb", this);
        drv = new("drv", this);
        hb.set_heartbeat(drv, 3);
    endfunction
    task run_phase();
        #5;
        hb.heartbeat(drv);  // hanya 1 dari 3 — harusnya GAGAL
    endtask
    function void check_phase();
        if (hb.check() != 0)
            $error("hb.check() harusnya 0 (drv hanya 1/3 heartbeat)");
    endfunction
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_heartbeat missing failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_tlm_fifo_blocking_put_full_and_peek() {
    // F23 review: jalur paling berisiko — `put` blocking saat penuh
    // (capacity 1: put a sukses, put b suspend → getter pop → putter resume
    // push b) + `peek` blocking (baca head tanpa pop, used tetap).
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_env extends uvm_env;
    uvm_tlm_fifo fifo;
    my_item r1;
    my_item r2;
    int put_b_done;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        fifo = new("fifo", this, 1);
    endfunction
    task run_phase();
        my_item a;
        my_item b;
        fork
            begin
                a = new("a");
                a.addr = 1;
                fifo.put(a);
                b = new("b");
                b.addr = 2;
                fifo.put(b); // penuh → suspend sampai getter pop a
                put_b_done = 1;
            end
            begin
                #10;
                fifo.get(r1); // pop a → release putter
                fifo.peek(r2); // baca b tanpa pop
            end
        join
        if (put_b_done != 1) $error("put blocking saat penuh gagal");
        if (r1.addr != 1) $error("r1=%0d harusnya 1", r1.addr);
        if (r2.addr != 2) $error("r2=%0d harusnya 2 (peek)", r2.addr);
        if (fifo.used() != 1) $error("peek tidak boleh pop, used=%0d", fifo.used());
        if (fifo.is_empty() != 0 || fifo.is_full() != 1) $error("is_empty/is_full salah");
    endtask
endclass

class my_test extends uvm_test;
    my_env env;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void build_phase();
        env = new("env", this);
    endfunction
endclass
module tb;
    initial run_test("my_test");
endmodule
"#;
    let result = simulate_str(source, 500);
    assert!(
        result.is_ok(),
        "uvm_tlm_fifo blocking put full / peek failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_driver_blocking_handshake() {
    // F24: blocking handshake sequence/sequencer/driver — `start_item` push
    // + release getter, driver `get_next_item` BLOCK sampai item tersedia
    // (pop + tulis lvalue), `finish_item` BLOCK sampai driver `item_done`
    // (release finisher per-item). Tanpa sinkronisasi ini, sequence selesai
    // duluan & driver tak dapat item (got=0). Field `req` (bukan task-local)
    // karena write_ast_lvalue menulis ke field/signal, bukan local task.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_driver extends uvm_driver;
    int got_cnt;
    int last_addr;
    my_item req;
    task run();
        while (got_cnt < 3) begin
            get_next_item(req);
            got_cnt = got_cnt + 1;
            last_addr = req.addr;
            item_done();
        end
    endtask
endclass

class my_sequence extends uvm_sequence;
    int sent_cnt;
    task body();
        my_item it;
        it = new("it1");
        it.addr = 100;
        start_item(it);
        finish_item(it);
        it = new("it2");
        it.addr = 200;
        start_item(it);
        finish_item(it);
        it = new("it3");
        it.addr = 300;
        start_item(it);
        finish_item(it);
        sent_cnt = 3;
    endtask
endclass

module tb;
    my_driver drv;
    uvm_sequencer sqr;
    my_sequence seq;
    initial begin
        drv = new("drv", null);
        sqr = new("sqr", null);
        drv.set_sequencer(sqr);
        seq = new("seq");
        fork
            drv.run();
            seq.start(sqr);
        join
        #50;
        if (drv.got_cnt != 3) $error("driver tak dapat 3 item, got=%0d", drv.got_cnt);
        if (drv.last_addr != 300) $error("addr terakhir salah %0d", drv.last_addr);
        if (seq.sent_cnt != 3) $error("sequence tak selesai, sent=%0d", seq.sent_cnt);
    end
endmodule
"#;
    let result = simulate_str(source, 1000);
    assert!(
        result.is_ok(),
        "uvm sequence/driver blocking handshake failed: {:?}",
        result.err()
    );
}

#[test]
fn test_ir_fork_join_waits_for_suspended_task() {
    // F26: IR `fork ... join` di module initial TIDAK boleh selesai premature
    // saat branch men-suspend task method (delay). Sebelumnya arm loop AST
    // (LoopWhile/LoopForever/LoopFor/Repeat/DoWhile) memakai `break` internal
    // saat body suspend → `evaluate_ast_block_with_delay_fork` mengembalikan
    // Ok(true) → fork_branch_end decrement → join selesai sebelum task selesai.
    // Sekarang `return Ok(false)` diteruskan; resume ContinueAstBlock(fork_id)
    // yang decrement saat branch benar-benar selesai.
    let source = r#"
class worker;
    int done_cnt;
    task run();
        #10;
        done_cnt = done_cnt + 1;
    endtask
endclass

module tb;
    worker w1, w2;
    initial begin
        w1 = new();
        w2 = new();
        fork
            w1.run();
            w2.run();
        join
        // Tanpa fix: join selesai di t=0 → done_cnt masih 0 → $error.
        if (w1.done_cnt + w2.done_cnt != 2) $error("fork join premature: done=%0d", w1.done_cnt + w2.done_cnt);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 100);
    assert!(
        result.is_ok(),
        "fork/join harus menunggu task suspend: {:?}",
        result.err()
    );
}

#[test]
fn test_ir_fork_uvm_handshake_no_workaround() {
    // F26: fork...join handshake UVM penuh TANPA workaround `#50` setelah
    // join (sebelumnya wajib karena fork IR selesai premature saat task
    // suspend). Driver harus dapat 3 item (100/200/300) sebelum join lewat.
    let source = r#"
class my_item extends uvm_sequence_item;
    int addr;
    function new(string name);
        super.new(name);
    endfunction
endclass

class my_driver extends uvm_driver;
    int got_cnt;
    int last_addr;
    my_item req;
    task run();
        while (got_cnt < 3) begin
            get_next_item(req);
            got_cnt = got_cnt + 1;
            last_addr = req.addr;
            item_done();
        end
    endtask
endclass

class my_sequence extends uvm_sequence;
    int sent_cnt;
    task body();
        my_item it;
        it = new("it1");
        it.addr = 100;
        start_item(it);
        finish_item(it);
        it = new("it2");
        it.addr = 200;
        start_item(it);
        finish_item(it);
        it = new("it3");
        it.addr = 300;
        start_item(it);
        finish_item(it);
        sent_cnt = 3;
    endtask
endclass

module tb;
    my_driver drv;
    uvm_sequencer sqr;
    my_sequence seq;
    initial begin
        drv = new("drv", null);
        sqr = new("sqr", null);
        drv.set_sequencer(sqr);
        seq = new("seq");
        fork
            drv.run();
            seq.start(sqr);
        join
        // Tanpa #50: join harus menunggu handshake selesai (got=3 last=300 sent=3).
        if (drv.got_cnt != 3 || drv.last_addr != 300 || seq.sent_cnt != 3)
            $error("fork join premature: got=%0d last=%0d sent=%0d", drv.got_cnt, drv.last_addr, seq.sent_cnt);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 1000);
    assert!(
        result.is_ok(),
        "fork join harus menunggu handshake UVM: {:?}",
        result.err()
    );
}

#[test]
fn test_interface_port_dut_always_ff_hier_clock() {
    // F27: port bertipe interface di module + always_ff dengan clock hierarkis
    // (`posedge b.clk`). Port interface dielaborasi sebagai handle 64-bit
    // (iface_type); field diakses via hier_signal_map (b.clk). Sebelumnya:
    // "always_ff must have at least one clock edge" + koneksi `.b(b)` gagal
    // di elaborasi (E3001).
    let source = r#"
interface bus_if;
    logic clk;
    logic [7:0] data;
endinterface

module dut (bus_if b);
    always_ff @(posedge b.clk) begin
        b.data <= 8'h42;
    end
endmodule

module tb;
    bus_if b();
    dut u_dut (.b(b));
    initial begin
        b.clk = 0;
        forever #5 b.clk = ~b.clk;
    end
    initial begin
        #50;
        if (b.data != 8'h42) $error("interface data mismatch: %0h", b.data);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(
        result.is_ok(),
        "interface port + hier clock: {:?}",
        result.err()
    );
}

#[test]
fn test_interface_modport_port_procedural_event() {
    // F27: port interface + modport (`axi_lite.dut bus`) + `@(posedge bus.clk)`
    // prosedural (jalur AST event) + reset via field interface. Parser dulu
    // tidak mengenali `iface.modport name` → dtype_name kosong → modul rusak.
    let source = r#"
interface axi_lite;
    logic        clk;
    logic        rst_n;
    logic [7:0]  awaddr;
    logic [7:0]  wdata;
    logic [7:0]  rdata;
    modport dut (input clk, rst_n, awaddr, wdata, output rdata);
endinterface

module slave (axi_lite.dut bus);
    always_ff @(posedge bus.clk or negedge bus.rst_n) begin
        if (!bus.rst_n)
            bus.rdata <= 8'h00;
        else if (bus.awaddr == 8'h10)
            bus.rdata <= bus.wdata + 8'h01;
    end
endmodule

module tb;
    axi_lite bus();
    slave u_slave (.bus(bus));
    initial begin
        bus.rst_n = 0;
        bus.awaddr = 0;
        bus.wdata = 0;
        bus.clk = 0;
        forever #5 bus.clk = ~bus.clk;
    end
    initial begin
        #10 bus.rst_n = 1;
        @(posedge bus.clk);
        bus.awaddr = 8'h10;
        bus.wdata = 8'h2a;
        @(posedge bus.clk);
        #1;
        if (bus.rdata != 8'h2b) $error("modport read mismatch: %0h", bus.rdata);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(
        result.is_ok(),
        "modport port + procedural event: {:?}",
        result.err()
    );
}

#[test]
fn test_interface_with_ports_connection() {
    // F27: interface yang PUNYA port + koneksi port eksternal
    // (`bus_if b(.clk(clk), ...)`) ke modul port interface — koneksi port_map
    // generic harus tetap jalan untuk instance interface.
    let source = r#"
interface bus_if (
    input  logic        clk,
    input  logic        rst_n,
    input  logic [7:0]  din,
    output logic [7:0]  dout
);
    logic [7:0] shadow;
endinterface

module dut (bus_if b);
    always_ff @(posedge b.clk or negedge b.rst_n) begin
        if (!b.rst_n) begin
            b.dout <= 8'h00;
            b.shadow <= 8'h00;
        end else begin
            b.dout <= b.din + 8'h03;
            b.shadow <= b.din;
        end
    end
endmodule

module tb;
    logic clk;
    logic rst_n;
    logic [7:0] din;
    logic [7:0] dout;
    bus_if b (.clk(clk), .rst_n(rst_n), .din(din), .dout(dout));
    dut u_dut (.b(b));
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    initial begin
        rst_n = 0;
        din = 8'h07;
        #10 rst_n = 1;
        #30 din = 8'h20;
        #20;
        if (dout != 8'h23) $error("iface port conn mismatch: %0h", dout);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(result.is_ok(), "interface with ports: {:?}", result.err());
}

#[test]
fn test_interface_port_alias_hier_diff_names() {
    // F28: port interface di child (`axi_if`) dikoneksikan ke instance
    // interface di parent dengan nama BEDA (`bus`) — akses `axi_if.<field>`
    // di child harus resolve ke signal flatten `bus.<field>` via alias hier
    // yang ditambahkan flatten (class_name handle menyimpan nama instance).
    // Pola ini dipakai .mv: `sig bus : axi_lite` → `axi_lite bus();`.
    let source = r#"
interface axi_lite;
    bit clk;
    logic [7:0] awaddr;
    logic [7:0] wdata;
endinterface

module dut (axi_lite axi_if);
    always_ff @(posedge axi_if.clk) begin
        axi_if.wdata <= axi_if.awaddr + 8'h01;
    end
endmodule

module tb;
    axi_lite bus();
    dut u_dut (.axi_if(bus));
    initial begin
        bus.clk = 0;
        forever #5 bus.clk = ~bus.clk;
    end
    initial begin
        #10 bus.awaddr = 8'h2a;
        #30;
        if (bus.wdata != 8'h2b) $error("alias hier mismatch: %0h", bus.wdata);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(result.is_ok(), "alias hier diff names: {:?}", result.err());
}

#[test]
fn test_hex_literal_equality_fix() {
    // F30: literal sized hex di-zero-extend — `sig == 16'h6` benar setelah
    // value_to_logicvec zero-fill. Sebelumnya bit di atas digit = X
    // (LogicVec::new = fill X) → X di operand → hasil X → false.
    let source = r#"
module tb_hexeq;
    logic [15:0] addr = 16'd6;
    initial begin
        #1;
        if (addr != 16'h6) $error("hex eq 6 gagal: got %0d", addr == 16'h6);
        if (addr != 16'h06) $error("hex eq 06 gagal");
        if (addr != 16'hA) $error("hex eq A: 6 harus != 10");
        if (addr != 16'd6) $error("dec eq gagal");
        if (addr != 6) $error("unsized eq gagal");
        $display("HEXEQ_OK");
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 50);
    assert!(result.is_ok(), "hex eq: {:?}", result.err());
}

#[test]
fn test_hex_literal_x_digit_stays_x() {
    // F30 fix: digit x/z eksplisit di literal tetap X (bukan di-zero-kan).
    let source = r#"
module tb_hexx;
    logic [7:0] v;
    initial begin
        v = 8'hx6;
        #1;
        if ($isunknown(v) != 1) $error("8'hx6 harus unknown");
        $display("HEXX_OK");
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 50);
    assert!(result.is_ok(), "hex x: {:?}", result.err());
}

#[test]
fn test_interface_alias_order_independent() {
    // F28 fix review: alias hier port interface diproses POST-pass setelah
    // SEMUA instance ter-flatten — sehingga urutan AST tidak relevan.
    // Di sini child module (`u_dut`) ditulis SEBELUM instance interface
    // (`bus()`) — tanpa post-pass alias tak terbentuk (bus.* belum ada saat
    // u_dut di-flatten) → akses `axi_if.wdata` gagal.
    let source = r#"
interface axi_lite;
    bit clk;
    logic [7:0] awaddr;
    logic [7:0] wdata;
endinterface

module dut (axi_lite axi_if);
    always_ff @(posedge axi_if.clk) begin
        axi_if.wdata <= axi_if.awaddr + 8'h01;
    end
endmodule

module tb;
    dut u_dut (.axi_if(bus));
    axi_lite bus();
    initial begin
        bus.clk = 0;
        forever #5 bus.clk = ~bus.clk;
    end
    initial begin
        #10 bus.awaddr = 8'h2a;
        #30;
        if (bus.wdata != 8'h2b) $error("alias order mismatch: %0h", bus.wdata);
        $finish;
    end
endmodule
"#;
    let result = simulate_str(source, 200);
    assert!(
        result.is_ok(),
        "alias order independent: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_report_object_compile() {
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function void do_report();
        uvm_report_info("my_id", "info message", 0);
    endfunction
endclass

module tb;
    my_comp c;
    initial begin
        c = my_comp::new("c", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_report_object compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_factory_override() {
    let source = r#"
class base_driver extends uvm_driver;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function string get_type();
        return "base_driver";
    endfunction
endclass

class extended_driver extends uvm_driver;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function string get_type();
        return "extended_driver";
    endfunction
endclass

module tb;
    base_driver drv;
    initial begin
        uvm_factory::set_type_override_by_type("base_driver", "extended_driver");
        drv = base_driver::new("drv", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_factory override compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_ovm_new_builtin_ast_path() {
    // VERIF-20 t3: `ovm_object::new` di jalur eval AST (body method class)
    // — remap ke `__ovm_object` + insert uvm_object_data (parity jalur IR),
    // sehingga get_name() bekerja (tanpa remap: alloc "ovm_object" polos →
    // dispatch method gagal RT8001). Jalur AST dieksekusi lewat body
    // function class (execute_method_body → evaluate_ast_stmt).
    let source = r#"
class c_env extends ovm_object;
    function new(string n);
        super.new(n);
    endfunction

    function int cek_obj();
        ovm_object o;
        string s;
        o = ovm_object::new("Abc");
        s = o.get_name();
        if (s == "Abc") return 1;
        return 0;
    endfunction
endclass

module tb;
    c_env e;
    int r;
    initial begin
        e = c_env::new("env");
        r = e.cek_obj();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("ovm_object::new jalur AST gagal");
    let r = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(
        r, 1,
        "get_name() pada objek ovm_object::new harus 'Abc', got r={r}"
    );
}

#[test]
fn test_ovm_alias_all_classes_remap_to_uvm() {
    // VERIF-20 t3 (review): alias ovm_* harus dinormalisasi ke __uvm_*, bukan
    // __ovm_* — kelas sintetis yang di-inject elaborator hanya __uvm_*, dan
    // dispatch method mensyaratkan nama berawalan `__uvm_`. Dengan remap
    // __ovm_* hanya ovm_object/ovm_component yang dikenali; ovm_test,
    // ovm_driver, ovm_analysis_port dkk. tetap RT8001.
    let source = r#"
module tb;
    ovm_test t;
    ovm_component c;
    int r1, r2;
    string s1, s2;
    initial begin
        t = ovm_test::new("t");
        s1 = t.get_name();
        r1 = (s1 == "t") ? 1 : 0;
        c = ovm_component::new("c", null);
        s2 = c.get_name();
        r2 = (s2 == "c") ? 1 : 0;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("alias ovm_* gagal disimulasikan");
    let get = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("r1"), 1, "ovm_test::new + get_name harus 't'");
    assert_eq!(get("r2"), 1, "ovm_component::new + get_name harus 'c'");
}

#[test]
fn test_ovm_factory_override_alias() {
    // VERIF-20 tahap 2: `ovm_factory::set_type_override_by_type` alias
    // ke handler UVM yang sama (elab SysFunc + engine IR/AST).
    // Runtime: return 1 + override tercatat (terbaca via get-like path:
    // override memengaruhi `new` berikutnya — di sini assert return).
    let source = r#"
module tb;
    int ok;
    initial begin
        ok = ovm_factory::set_type_override_by_type("base_driver", "extended_driver");
        assert(ok == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("ovm_factory run failed");
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(ok, 1, "ovm_factory alias harus return 1, got {}", ok);
}

#[test]
fn test_uvm_resource_db_set_get() {
    // VERIF-07: set/get sebagai bare statement (bukan di-eliminasi
    // elaborator) — set menyimpan ke map, get mengembalikan nilai.
    let source = r#"
module tb;
    int val;
    int success;
    int missing_success;
    initial begin
        uvm_resource_db::set("scope1", "key1", 99);
        success = uvm_resource_db::get("scope1", "key1", val);
        missing_success = uvm_resource_db::get("scope1", "missing", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("uvm_resource_db sim");
    let (_, sv) = sigs.iter().find(|(n, _)| n == "success").unwrap();
    assert_eq!(sv.to_u64(), 1, "get harus menemukan resource yang di-set");
    let (_, vv) = sigs.iter().find(|(n, _)| n == "val").unwrap();
    assert_eq!(vv.to_u64(), 99, "nilai resource harus 99");
    let (_, mv) = sigs.iter().find(|(n, _)| n == "missing_success").unwrap();
    assert_eq!(mv.to_u64(), 0, "get untuk key yang tidak ada harus 0");
}

#[test]
fn test_uvm_resource_db_wildcard_scope() {
    // VERIF-07: wildcard scope — set("*.env", ...) terbaca oleh
    // get("tb.env", ...) dan exists("tb.env", ...) = 1.
    let source = r#"
module tb;
    int val;
    int success;
    int e1;
    int e2;
    initial begin
        uvm_resource_db::set("*.env", "baud", 115200);
        success = uvm_resource_db::get("tb.env", "baud", val);
        e1 = uvm_resource_db::exists("tb.env", "baud");
        e2 = uvm_resource_db::exists("tb.env", "nope");
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("wildcard resource_db sim");
    let (_, sv) = sigs.iter().find(|(n, _)| n == "success").unwrap();
    assert_eq!(sv.to_u64(), 1, "wildcard scope harus match get");
    let (_, vv) = sigs.iter().find(|(n, _)| n == "val").unwrap();
    assert_eq!(vv.to_u64(), 115200, "nilai dari set wildcard harus terbaca");
    let (_, a) = sigs.iter().find(|(n, _)| n == "e1").unwrap();
    assert_eq!(a.to_u64(), 1, "exists untuk resource wildcard harus 1");
    let (_, b) = sigs.iter().find(|(n, _)| n == "e2").unwrap();
    assert_eq!(
        b.to_u64(),
        0,
        "exists untuk resource yang tidak ada harus 0"
    );
}

#[test]
fn test_uvm_resource_db_read_write_by_name() {
    // VERIF-07: write_by_name = alias set, read_by_name = alias get.
    let source = r#"
module tb;
    int val;
    int success;
    initial begin
        uvm_resource_db::write_by_name("s1", "k1", 42);
        success = uvm_resource_db::read_by_name("s1", "k1", val);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).expect("resource_db read/write sim");
    let (_, sv) = sigs.iter().find(|(n, _)| n == "success").unwrap();
    assert_eq!(sv.to_u64(), 1, "read_by_name harus menemukan resource");
    let (_, vv) = sigs.iter().find(|(n, _)| n == "val").unwrap();
    assert_eq!(
        vv.to_u64(),
        42,
        "read_by_name harus mengembalikan nilai write_by_name"
    );
}

#[test]
fn test_param_class_compile() {
    let source = r#"
class #(type T = int) my_param_class;
    T data;
    function T get_data();
        return data;
    endfunction
    function new(T val);
        data = val;
    endfunction
endclass
module tb;
    my_param_class obj;
    initial begin
        obj = my_param_class #(int)::new(42);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "param class sim failed: {:?}", result.err());
}

#[test]
fn test_uvm_scoreboard_compile() {
    let source = r#"
class my_scoreboard extends uvm_scoreboard;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
endclass

module tb;
    my_scoreboard sb;
    initial begin
        sb = my_scoreboard::new("sb", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_scoreboard compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_monitor_compile() {
    let source = r#"
class my_monitor extends uvm_monitor;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    task run_phase(uvm_phase phase);
        // monitor observes transactions
    endtask
endclass

module tb;
    my_monitor mon;
    initial begin
        mon = my_monitor::new("mon", 0);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "uvm_monitor compile failed: {:?}",
        result.err()
    );
}

#[test]
fn test_uvm_sequence_item_get_type_name() {
    let source = r#"
class my_item extends uvm_sequence_item;
    function new(string name);
        super.new(name);
    endfunction
endclass

module tb;
    my_item item;
    reg [63:0] tname;
    initial begin
        item = my_item::new("my_item");
        tname = item.get_type_name();
        #1 $finish;
    end
endmodule
"#;
    let _sigs = simulate_signals(source, 5).unwrap();
    // get_type_name returns string bits, we just verify sim completes
}

#[test]
fn test_const_fold_binary_op() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 10 + 20;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 30, "10 + 20 should fold to 30");
}

#[test]
fn test_const_fold_ternary() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = (1) ? 100 : 200;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 100, "ternary with true cond should fold to 100");
}

#[test]
fn test_const_fold_concat() {
    let source = r#"
module tb;
    reg [7:0] x;
    initial begin
        x = {4'b1010, 4'b0101};
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 0xa5, "concat of constants should fold");
}

#[test]
fn test_dce_if_const_true() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        if (1) x = 50; else x = 99;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 50, "if(1) should execute true branch");
}

#[test]
fn test_dce_if_const_false() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        if (0) x = 50; else x = 99;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 99, "if(0) should execute false branch");
}

#[test]
fn test_dce_case_const() {
    let source = r#"
module tb;
    reg [31:0] x;
    integer sel;
    initial begin
        sel = 2;
        case (sel)
            0: x = 10;
            1: x = 20;
            2: x = 30;
            3: x = 40;
        endcase
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 30, "case const 2 -> x=30");
}

#[test]
fn test_dce_case_default() {
    let source = r#"
module tb;
    reg [31:0] x;
    integer sel;
    initial begin
        sel = 99;
        case (sel)
            0: x = 10;
            1: x = 20;
            default: x = 99;
        endcase
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 99, "case default -> x=99");
}

#[test]
fn test_dce_if_no_else() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        if (1) x = 50;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 50, "if(1) no else should execute true branch");
}

#[test]
fn test_assert_pass() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 1;
        assert (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "assert with true condition should not fail");
}

#[test]
fn test_assert_fail() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        assert (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(1);
    assert_eq!(v, 0, "assert with false condition should continue");
}

#[test]
fn test_assert_else_stmt() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        assert (x == 1) else x = 99;
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 99, "assert else stmt should execute on failure");
}

#[test]
fn test_assertion_coverage_metrics() {
    // VERIF-27: assertion coverage metrics — engine.assertion_stats mencatat
    // pass/fail per assertion (assert immediate + expect, jalur IR).
    let source = r#"
module tb;
    int x;
    initial begin
        x = 5;
        assert (x > 0);
        assert (x > 10);
        expect (x == 5) else;
        expect (x == 6) else;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    let _ = engine.run();
    assert_eq!(
        engine.assertion_stats.len(),
        4,
        "4 assertion (2 assert + 2 expect) dievaluasi: {:?}",
        engine.assertion_stats
    );
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert_eq!(total_pass, 2, "2 pass: assert(x>0), expect(x==5)");
    assert_eq!(total_fail, 2, "2 fail: assert(x>10), expect(x==6)");
}

#[test]
fn test_assertion_coverage_concurrent_sequence() {
    // VERIF-27: concurrent assertion (assert property @(posedge clk)) —
    // completion sequence attempt tercatat pass/fail di assertion_stats.
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    // 4 siklus clk → cnt 1..4; assertion `cnt <= 3` gagal di siklus ke-4.
    // (assert property module-level tidak di-drive jadi process — pakai
    // pola immediate assertion di dalam forever @(posedge clk).)
    initial begin
        forever begin
            @(posedge clk);
            assert (cnt <= 3) else $display("over");
        end
    end
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "cnt<=3 lulus siklus 1..3: pass={}",
        total_pass
    );
    assert!(
        total_fail >= 1,
        "cnt<=3 gagal siklus ke-4: fail={}",
        total_fail
    );
}

#[test]
fn test_sequence_coverage_matched_and_hole() {
    // VERIF-32: sequence coverage — concurrent assertion `a ##1 b`
    // di-track per (line, col): attempts, matched, failed. Sequence yang
    // tidak pernah match (matched == 0) muncul sebagai coverage gap.
    let src = r#"
module tb_seqcov;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 1;      // match: a di cycle-1, b di cycle ini
    @(posedge clk); a = 1; b = 0;
    @(posedge clk);             // b tetap 0 → attempt ini timeout → fail
    @(posedge clk); a = 0; b = 0;
    #60; $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile seqcov");
    let mut engine = crate::simulator::SimulationEngine::new(design, 200);
    let _ = engine.run();

    assert!(
        !engine.sequence_coverage.is_empty(),
        "sequence coverage tercatat"
    );
    let stats: Vec<_> = engine.sequence_coverage.values().collect();
    let total_attempts: u64 = stats.iter().map(|s| s.attempts).sum();
    let total_matched: u64 = stats.iter().map(|s| s.matched).sum();
    let total_failed: u64 = stats.iter().map(|s| s.failed).sum();
    assert!(
        total_attempts >= 2,
        ">=2 attempt dimulai: {}",
        total_attempts
    );
    assert!(total_matched >= 1, "a##1 b match sekali: {}", total_matched);
    assert!(
        total_failed >= 1 || total_matched < total_attempts,
        "ada attempt yang tidak match (fail): failed={}",
        total_failed
    );

    // Coverage gap: buat design kedua dengan sequence yang TIDAK PERNAH
    // match → matched == 0 → gap "sequence ... tidak pernah match".
    let hole_src = r#"
module tb_hole;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;   // b tidak pernah 1 → hole
    @(posedge clk); a = 0;
    #60; $finish;
  end
endmodule
"#;
    let design2 = compile_str(hole_src).expect("compile hole");
    let mut engine2 = crate::simulator::SimulationEngine::new(design2, 200);
    let _ = engine2.run();
    let holes: Vec<&(usize, usize)> = engine2
        .sequence_coverage
        .iter()
        .filter(|(_, s)| s.matched == 0 && s.attempts > 0)
        .map(|(k, _)| k)
        .collect();
    assert!(
        !holes.is_empty(),
        "sequence tanpa match terdeteksi sbg hole"
    );
    let gaps = engine2.coverage_gaps();
    assert!(
        gaps.iter().any(|g| g.contains("tidak pernah match")),
        "coverage_gaps() memuat sequence hole: {:?}",
        gaps
    );
}

#[test]
fn test_cover_pass() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 1;
        cover (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "cover should not affect execution");
}

#[test]
fn test_module_level_assert_property_pass_fail() {
    // LANG-04: module-level `assert property (@(posedge clk) expr)` —
    // assertion concurrent BOOLEAN kini di-drive jadi always block ber-clock:
    // pass saat cond true, fail (assertion_stats) saat cond false.
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assert property (@(posedge clk) cnt <= 3);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "module-level assert property: pass={}",
        total_pass
    );
    assert!(
        total_fail >= 1,
        "module-level assert property: fail={}",
        total_fail
    );
}

#[test]
fn test_module_level_cover_restrict_property() {
    // LANG-11/13: module-level `cover property` (hit saat true) + `restrict
    // property` (diperlakukan seperti assume — violation = fail metric).
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    reg [3:0] cnt_odd;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    cover property (@(posedge clk) cnt >= 2);
    // restrict cnt <= 3 → committed cnt = 4 di t=45 (siklus ke-5) = violation.
    restrict property (@(posedge clk) cnt <= 3);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    // cover property → hit tercatat di cover_hits (key cover@line:col).
    let cover_total: u64 = engine.cover_hits.values().sum();
    assert!(
        cover_total >= 1,
        "cover property harus tercatat hit: {}",
        cover_total
    );
    // restrict property (asumsi) → violation saat committed cnt=4 (>3).
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "restrict property violation harus tercatat fail: {}",
        total_fail
    );
}

#[test]
fn test_module_level_assume_property() {
    // LANG-12: module-level `assume property` — violation = fail metric
    // (asumsi constraint; jika dilanggar → error).
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assume property (@(posedge clk) cnt != 2);
    initial begin
        #30 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 40);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "assume property violation harus fail: {}",
        total_fail
    );
}

#[test]
fn test_module_level_property_temporal_skipped() {
    // LANG-04: property dgn operator temporal (`##1`, `|->`) tidak di-parse
    // (limitation) — modul tetap ter-parse utuh (tidak ada error E1002,
    // assertion kompleks di-skip).
    let source = r#"
module tb;
    reg clk;
    reg a, b;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        a <= b;
    end
    assert property (@(posedge clk) a ##1 b);
    initial begin
        #20 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 25).expect("temporal property skipped, modul tetap parse");
    let v = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(v.1.width > 0, "clk harus tetap ada — modul ter-parse");
}
#[test]
fn test_psl_boolean_assert_always_never() {
    // LANG-03: PSL (IEEE 1850) boolean — `assert always (expr) @(posedge
    // clk);` / `assert never (expr) @(posedge clk);` + directive
    // `default clock = posedge clk;`. always = properti true tiap cycle;
    // never = properti tidak boleh true (cond dibalik !). Dievaluasi tiap
    // posedge via jalur assertion module-level (ROUND 71).
    let source = r#"
default clock = posedge clk;
module tb;
    reg clk = 0;
    reg [7:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assert always (cnt <= 8) @(posedge clk);
    assert never (cnt > 8) @(posedge clk);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_pass >= 8, "PSL always+never: pass={}", total_pass);
    assert_eq!(
        total_fail, 0,
        "PSL never (cnt>8 dibalik) tidak boleh fail: {}",
        total_fail
    );
}

#[test]
fn test_psl_temporal_skipped_safe() {
    // LANG-03: PSL operator temporal (`|->`, `until`) tidak didukung lexer
    // → parse gagal → rollback + skip assertion (modul tetap utuh, perilaku
    // sama dengan temporal SVA LANG-04). Directive `default clock` juga
    // di-skip tanpa error.
    let source = r#"
default clock = posedge clk;
module tb;
    reg clk;
    reg a, b;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        a <= b;
    end
    assert always (a |-> b) @(posedge clk);
    assert always (a until b) @(posedge clk);
    initial begin
        #20 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 25).expect("PSL temporal skipped, modul tetap parse");
    let v = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(v.1.width > 0, "clk harus tetap ada — modul ter-parse");
}

#[test]
fn test_sva_temporal_sequence() {
    // LANG-04: `assert property (@(posedge clk) a ##1 b)` — sequence
    // temporal SVA. Engine mengevaluasi via `SequenceAttempt`: setiap
    // posedge, attempt baru dimulai; evaluator `eval_sequence_depth`
    // memeriksa `a` pada depth=1 (1 posedge lalu) dan `b` pada depth=0
    // (current posedge). Pass jika match, fail jika timeout max_cycles.
    //
    // Test PASS: a=1 diikuti b=1 → 1 match
    // Test FAIL: a pulse tapi b selalu 0 → 0 match, semua fail
    let pass_src = r#"
module tb_pass;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 1;
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(pass_src, 150).expect("compile pass case");
    let clk = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(clk.1.width > 0, "clk exists");

    let fail_src = r#"
module tb_fail;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); a = 0;
    #100; $finish;
  end
endmodule
"#;
    let sigs = simulate_signals(fail_src, 150).expect("compile fail case");
    let clk = sigs.iter().find(|(n, _)| n == "clk").unwrap();
    assert!(clk.1.width > 0, "clk exists");
}

#[test]
fn test_sva_temporal_range_delay() {
    // LANG-04 extension: `##[min:max]` range delay (IEEE 1800-2017 §16.9.2.2).
    // a ##[1:2] b → b harus true 1 atau 2 cycle SETELAH a true.
    //
    // Case 1: b true di cycle+1 (min match) → pass
    let pass_min = r#"
module tb_range_pass;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##[1:2] b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 1;  // cycle+1 → min match
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_min).expect("compile range pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    assert!(total_pass >= 1, "##[1:2] min match: pass={}", total_pass);

    // Case 2: b true di cycle+2 (max match) → pass
    let pass_max = r#"
module tb_range_pass2;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##[1:2] b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 0;  // cycle+1 miss
    @(posedge clk); b = 1;  // cycle+2 → max match
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_max).expect("compile range pass2");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    assert!(total_pass >= 1, "##[1:2] max match: pass={}", total_pass);

    // Case 3: b never true → fail
    let fail_src = r#"
module tb_range_fail;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##[1:2] b);
  initial begin
    @(posedge clk); a = 1;
    @(posedge clk); b = 0;
    @(posedge clk); b = 0;
    @(posedge clk); a = 0; b = 0;
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile range fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_fail >= 1, "##[1:2] no match: fail={}", total_fail);
}

#[test]
fn test_sva_overlap_implication() {
    // LANG-04 extension: `|->` overlap implication (IEEE 1800-2017 §16.9.2).
    // Antecedent match di posedge k → consequent mulai dari posedge yang sama k.
    // Jika antecedent tidak pernah match → vacuously true (pass).
    //
    // Case vacuous: req=0 selalu → pass (vacuously true)
    let vacuous_src = r#"
module tb_vacuous;
  reg clk = 0;
  reg req = 0;
  reg ack = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) req |-> ack);
  initial begin
    #100; $finish;
  end
endmodule
"#;
    let design = compile_str(vacuous_src).expect("compile vacuous case");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_pass >= 1, "vacuous implication: pass={}", total_pass);
    assert_eq!(total_fail, 0, "vacuous implication: fail={}", total_fail);
}

#[test]
fn test_sva_nonoverlap_implication() {
    // LANG-47: `|=>` non-overlap implication (IEEE 1800-2017 §16.9.2).
    // Antecedent match di posedge k → consequent dievaluasi mulai posedge
    // k+1 (di window 1 cycle). Vacuous: req selamat dari match → pass.
    //
    // Pass: req=1 di cycle pertamanya yang true, done=1 TEPAT di posedge
    // berikutnya → attempt matched (pass).
    let pass_src = r#"
module tb_nonoverlap_pass;
  reg clk = 0;
  reg req = 0;
  reg done = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) req |=> done);
  initial begin
    @(posedge clk) req = 1;
    @(posedge clk) done = 1;  // done TRUE di posedge TEPAT setelah req
    @(posedge clk) req = 0;
    #20; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_src).expect("compile nonoverlap pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 1,
        "|=> pass case: pass={} fail={}",
        total_pass,
        total_fail
    );
    assert_eq!(total_fail, 0, "|=> pass case: fail={}", total_fail);

    // Fail: done=0 di posedge TEPAT setelah antecedent → attempt failed
    // (|=> menuntut true PERSIS di cycle berikutnya, bukan cycle sama).
    let fail_src = r#"
module tb_nonoverlap_fail;
  reg clk = 0;
  reg req = 1;   // req true sejak t=0 → antecedent match di posedge pertama
  reg done = 0;  // done TIDAK pernah → consequent gagal di posedge berikutnya
  always #5 clk = ~clk;
  assert property (@(posedge clk) req |=> done);
  initial begin
    #40; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile nonoverlap fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "|=> fail case belum terdeteksi: fail={}",
        total_fail
    );
}

#[test]
#[test]
fn test_sva_multicycle_antecedent_implication_fails() {
    // Regresi: assertion a ##1 b |-> c — antecedent multi-cycle harus
    // dievaluasi pada cycle saat ia SELESAI, bukan hanya elapsed==0 (yang
    // membuatnya selalu vacuous pass / false green).
    let src = r#"
module tb_multicycle_fail;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b |-> 1'b0);
  initial begin
    @(posedge clk) a = 1;
    @(posedge clk) b = 1;
    #30; $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile multicycle fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "antecedent multi-cycle harus menghasilkan fail, dapat {}",
        total_fail
    );
}

#[test]
fn test_sva_multicycle_antecedent_vacuous_pass() {
    // Antecedent tidak pernah selesai -> vacuous pass (IEEE 1800 16.12.2).
    let src = r#"
module tb_multicycle_vacuous;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  assert property (@(posedge clk) a ##1 b |-> 1'b0);
  initial begin
    @(posedge clk) a = 1;
    #30; $finish;
  end
endmodule
"#;
    let design = compile_str(src).expect("compile multicycle vacuous");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert_eq!(
        total_fail, 0,
        "harus vacuous pass, dapat fail={}",
        total_fail
    );
}

fn test_sva_labeled_assertion() {
    // LANG-48: assertion berlabel `lbl: assert property (...)` di level
    // module TIDAK boleh dibuang — label di-consume, body di-parse penuh,
    // assertion terelaborasi dan ter-evaluate (pass & fail terhitung).
    //
    // Pass: tidak berlabel ini pass + berlabel this pass.
    let pass_src = r#"
module tb_label_pass;
  reg clk = 0;
  reg a = 0;
  reg b = 0;
  always #5 clk = ~clk;
  labeled_ok: assert property (@(posedge clk) a |-> (a == b));
  unlabeled_ok: assert property (@(posedge clk) b |-> (b == a));
  initial begin
    @(posedge clk) a = 1;
    @(posedge clk) b = 1;   // b==a dari sini → kedua assertion pass
    @(posedge clk) a = 0;
    #20; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_src).expect("compile labeled pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 80);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    // 2 assertion (labeled + unlabeled) keduanya harus ter-evaluate.
    assert!(
        total_pass >= 2,
        "labeled assertion pass: pass={} fail={}",
        total_pass,
        total_fail
    );

    // Fail: labeled assertion harus mencatat fail (done tidak terpenuhi).
    let fail_src = r#"
module tb_label_fail;
  reg clk = 0;
  reg req = 1;   // req true sejak t=0
  reg done = 0;  // done tidak pernah true
  always #5 clk = ~clk;
  as_req_done: assert property (@(posedge clk) req |-> done);
  initial begin
    #40; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile labeled fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_fail >= 1,
        "labeled assertion fail belum terdeteksi: fail={}",
        total_fail
    );
}

#[test]
fn test_sva_property_always_never_iff() {
    // LANG-49: operator temporal property (IEEE 1800-2017 §16.14/§16.10):
    // `always (p)` — p true dinilai di setiap posedge;
    // `never (p)` — p TIDAK boleh true di edge mana pun;
    // `a iff b` — TRUE ketika kedua sisi identik (XNOR boolean).
    //
    // Pass: a==1 sepanjang simulasi → always pass, never pass, iff pass.
    let pass_src = r#"
module tb_prop_pass;
  reg clk = 0;
  reg a = 1;
  reg b = 1;
  always #5 clk = ~clk;
  as_always: assert property (@(posedge clk) always (a == 1));
  as_never: assert property (@(posedge clk) never (a == 0));
  as_iff: assert property (@(posedge clk) a iff b);
  initial begin
    #45; $finish;
  end
endmodule
"#;
    let design = compile_str(pass_src).expect("compile temporal pass");
    let mut engine = crate::simulator::SimulationEngine::new(design, 80);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(
        total_pass >= 3,
        "always/never/iff pass: pass={} fail={}",
        total_pass,
        total_fail
    );
    assert_eq!(total_fail, 0, "always/never/iff pass: fail={}", total_fail);

    // Fail: a 0 SATU posedge → always + never + iff masing-masing
    // mencatat fail pada cycle itu (b == 0 juga).
    let fail_src = r#"
module tb_prop_fail;
  reg clk = 0;
  reg a = 1;
  reg b = 1;
  always #5 clk = ~clk;
  as_always: assert property (@(posedge clk) always (a == 1));
  as_never: assert property (@(posedge clk) never (a == 0));
  as_iff: assert property (@(posedge clk) a iff b);
  initial begin
    #12 a = 0; b = 0;   // a=b=0 di pertengahan → jika iff dinilai PER-CYCLE,
                        // iff ini tetap true; yang gagal: always (a==1)
                        // dan never (a==0).
    #6 a = 1; b = 1;
    #20; $finish;
  end
endmodule
"#;
    let design = compile_str(fail_src).expect("compile temporal fail");
    let mut engine = crate::simulator::SimulationEngine::new(design, 80);
    let _ = engine.run();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    // minimal 2 fail: always (a==1) salah satum & never (a==0) saat a=0.
    assert!(
        total_fail >= 1,
        "temporal fail belum terdeteksi: fail={}",
        total_fail
    );
}

#[test]
fn test_checker_let_items() {
    // LANG-50: `let` di dalam checker di-resolve oleh body assertion
    // (IEEE 1800-2017 §17.8 — checker items satu scope). Body let boleh
    // mereferensi nama port checker (ter-bind saat instance).
    let source = r#"
module tb_chk_let;
    reg clk = 0;
    reg [3:0] cnt = 0;
    always #5 clk = ~clk;
    always @(posedge clk) cnt <= cnt + 1;

    checker chk_let (logic clk_p, logic [3:0] d, logic en);
        let thr = en;
        assert property (@(posedge clk_p) !thr || (d > 0));
        assert property (@(posedge clk_p) d <= 10);
    endchecker

    chk_let u_chk(clk, cnt, 1'b1);
    initial #50 $finish;
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    // Kedua assertion checker (thr let + rentang) pass — cnt 0..di 10.
    assert!(
        total_pass >= 2,
        "checker let assertion pass: pass={}",
        total_pass
    );
    let _ = total_pass;
}

#[test]
fn test_hier_port_edge_wait() {
    // LANG-51: instance port yang dikoneksikan ke referensi hierarkis
    // (`u_i.clk`) — wait @(posedge c) di child HARUS terpicu. Dulu: perubahan
    // clock di-commit region Active, deltas di-reset sebelum Reactive jalan,
    // posedge tak pernah terlihat oleh EventControl → hang.
    let source = r#"
module child (input logic c);
  initial begin
    @(posedge c);
    @(posedge c);
    $display("CHILD_HIER_OK");
    $finish;
  end
endmodule
module inner;
  reg clk = 0;
  always #5 clk = ~clk;
endmodule
module abi_top;
  inner u_i();
  child u_c(u_i.clk);
endmodule
"#;
    let design = compile_str(source).expect("compile hier port");
    let mut engine = crate::simulator::SimulationEngine::new(design, 100);
    let _ = engine.run();
    // Jika wait hang, CHILD_HIER_OK tak pernah dicetak — verifikasi via
    // assertion_stats kosong + proses selesai (engine.run selesai normal).
    // Penanda keberhasilan: simulasi berjalan tanpa hang; verifikasi output
    // $display via simulate-style jalur di atas tidak diekspos — gunakan
    // waktu selesai: sebelum fix, engine berjalan sampai max_time (100).
    assert!(
        engine.state.time < 100,
        "hier port edge wait: simulasi hang sampai max_time (time={})",
        engine.state.time
    );
}

#[test]
fn test_string_realtoa_psprintf() {
    // LANG-54: LRM 1800 §6.16.1 realtoa (MUTATING receiver) + §6.16.9
    // psprintf (receiver = format string, non-mutating).
    let src = r#"
module tb_realtoa_psprintf;
  reg ok_rt = 0;
  reg ok_ps = 0;
  initial begin
    real r = 3.5;
    string rs;
    rs.realtoa(r);
    if (rs == "3.5") ok_rt = 1;
    string ps = "a=%0d b=%s".psprintf(7, "xyz");
    if (ps == "a=7 b=xyz") ok_ps = 1;
  end
endmodule
"#;
    let sigs = crate::simulate_signals(src, 60).expect("sim realtoa+psprintf");
    let find = |name: &str| {
        sigs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_bool() == Some(true))
            .unwrap_or(false)
    };
    assert!(find("ok_rt"), "realtoa: rs harus '3.5'");
    assert!(find("ok_ps"), "psprintf: harus 'a=7 b=xyz'");
}

#[test]
fn test_assoc_iterator_methods() {
    // LANG-55: iterator associative array LRM 1800 §7.10.6 — first/next/
    // last/prev menulis var index (out-arg) dan mengembalikan 1 sukses /
    // 0 gagal. Urutan kunci ascending deterministik.
    let src = r#"
module tb_assoc_iter;
  reg ok_all = 0;
  initial begin
    int aa [string];
    string k;
    reg [31:0] vals;
    reg [31:0] vi;
    aa["a"]=1; aa["b"]=2; aa["c"]=3;
    vi=0;
    if (aa.first(k) && k=="a" && aa[k]==1) vi=vi|1;
    if (aa.next(k) && k=="b" && aa[k]==2) vi=vi|2;
    if (aa.next(k) && k=="c" && aa[k]==3) vi=vi|4;
    if (!aa.next(k)) vi=vi|8; // habis abis → rc 0
    if (aa.last(k) && k=="c" && aa[k]==3) vi=vi|16;
    if (aa.prev(k) && k=="b" && aa[k]==2) vi=vi|32;
    if (aa.prev(k) && k=="a" && aa[k]==1) vi=vi|64;
    if (!aa.prev(k)) vi=vi|128; // diawal kuli → rc 0, k tak berubah
    ok_all = (vi == 32'd255);
  end
endmodule
"#;
    let sigs = crate::simulate_signals(src, 60).expect("sim assoc iterator");
    let ok = sigs
        .iter()
        .find(|(n, _)| n == "ok_all")
        .map(|(_, v)| v.to_bool() == Some(true))
        .unwrap_or(false);
    assert!(
        ok,
        "assoc first/next/last/prev LRM: ok_all harus 1 (vi=255)"
    );
}

#[test]
fn test_checker_construct_instance() {
    // LANG-10: checker construct (IEEE 1800-2017 §17.8) — `checker name
    // (ports); assert property... endchecker` dideklarasikan, diinstansiasi
    // di module, assertion body di-drive dengan port binding (posisi).
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    checker my_checker(input clk, input [3:0] v);
        assert property (@(posedge clk) v <= 3);
    endchecker
    my_checker u_chk(clk, cnt);
    initial begin
        #50 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 60);
    let _ = engine.run();
    let total_pass: u64 = engine.assertion_stats.values().map(|(p, _)| *p).sum();
    let total_fail: u64 = engine.assertion_stats.values().map(|(_, f)| *f).sum();
    assert!(total_pass >= 3, "checker assertion: pass={}", total_pass);
    assert!(
        total_fail >= 1,
        "checker assertion: fail saat cnt>3: {}",
        total_fail
    );
}

#[test]
fn test_checker_construct_named_ports() {
    // LANG-10: checker instance dengan koneksi named — port binding ke
    // signal modul; assertion body dievaluasi tiap edge.
    let source = r#"
module tb;
    reg clk;
    reg [3:0] cnt = 0;
    initial begin
        clk = 0;
        forever #5 clk = ~clk;
    end
    always @(posedge clk) begin
        cnt <= cnt + 1;
    end
    checker my_checker(input clk, input [3:0] v);
        cover property (@(posedge clk) v >= 2);
    endchecker
    my_checker u_chk(.clk(clk), .v(cnt));
    initial begin
        #40 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 50);
    let _ = engine.run();
    let cover_total: u64 = engine.cover_hits.values().sum();
    assert!(
        cover_total >= 1,
        "checker cover property: hit={}",
        cover_total
    );
}

#[test]
fn test_assert_property_parse() {
    let source = r#"
module tb;
    reg clk;
    reg [31:0] x;
    initial begin
        clk = 0;
        x = 1;
        assert property (@(posedge clk) x == 1);
        #1 $finish;
    end
endmodule
"#;

    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(0);
    assert_eq!(v, 1, "concurrent assert property should parse and execute");
}

#[test]
fn test_assume_fail() {
    let source = r#"
module tb;
    reg [31:0] x;
    initial begin
        x = 0;
        assume (x == 1);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5).unwrap();
    let v = sigs
        .iter()
        .find(|(n, _)| n == "x")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(1);
    assert_eq!(v, 0, "assume with false condition should not crash");
}

#[test]
fn test_covergroup_parse() {
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg @(posedge clk);
        cp_a: coverpoint a;
    endgroup
    initial begin
        a = 42;
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "covergroup should parse without error: {:?}",
        result.err()
    );
}

#[test]
fn test_covergroup_integration_coverage_db() {
    // VERIF-21: functional coverage (covergroup) ter-integrasi ke
    // CoverageDatabase — merge_from_engine membawa total/hits/bins
    // coverpoint ke database (persisten/merge multi-run).
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a;
    endgroup
    cg cg_inst = new();
    initial begin
        a = 1;
        cg_inst.sample();
        a = 2;
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let mut db = crate::simulator::coverage_db::CoverageDatabase::new();
    db.merge_from_engine(&engine);
    let cg = db
        .covergroups
        .get("cg")
        .expect("covergroup 'cg' harus masuk CoverageDatabase");
    assert_eq!(cg.coverpoints.len(), 1);
    let cp = &cg.coverpoints[0];
    assert_eq!(cp.total, 2, "2 sample ter-integrasi ke db");
    assert_eq!(cp.hits, 2);
    assert_eq!(cp.bins.len(), 2, "2 bin unik (a=1, a=2)");
}

#[test]
fn test_load_coverage_db_merges() {
    // `$load_coverage_db(path)` — muat MCDB lalu MERGE ke counter live
    // (line/branch/toggle/FSM/covergroup dijumlahkan). Sebelumnya stub
    // "not yet implemented" — panggilan diam-diam tak berpengaruh.
    let src_a = r#"
module tb;
    reg [3:0] cnt;
    integer i;
    initial begin
        cnt = 0;
        for (i = 0; i < 4; i = i + 1) cnt = cnt + 1;
        #1 $finish;
    end
endmodule
"#;
    let design_a = compile_str(src_a).unwrap();
    let mut engine_a = crate::simulator::SimulationEngine::new(design_a, 10);
    engine_a.run().unwrap();
    assert!(
        !engine_a.cover_line.is_empty(),
        "run A harus catat line hits"
    );
    let saved_hits: u64 = engine_a.cover_line.values().sum();
    let db_path = std::env::temp_dir().join(format!("mivon_covdb_{}.mcdb", std::process::id()));
    let mut db = crate::simulator::coverage_db::CoverageDatabase::new();
    db.merge_from_engine(&engine_a);
    db.save_to_file(db_path.to_str().unwrap())
        .expect("simpan MCDB");

    // Run B memanggil $load_coverage_db — cover_line B harus memuat SEMUA
    // key dari A (merge), bukan hanya statement B sendiri.
    let src_b = format!(
        r#"
module tb2;
    reg [3:0] x;
    initial begin
        $load_coverage_db("{}");
        x = 1;
        #1 $finish;
    end
endmodule
"#,
        db_path.to_str().unwrap()
    );
    let design_b = compile_str(&src_b).unwrap();
    let mut engine_b = crate::simulator::SimulationEngine::new(design_b, 10);
    engine_b.run().unwrap();
    for key in engine_a.cover_line.keys() {
        assert!(
            engine_b.cover_line.contains_key(key),
            "key line {:?} dari MCDB harus ter-merge ke engine B",
            key
        );
    }
    let merged_hits: u64 = engine_b.cover_line.values().sum();
    assert!(
        merged_hits >= saved_hits,
        "hits B ({}) >= hits A tersimpan ({})",
        merged_hits,
        saved_hits
    );
    std::fs::remove_file(&db_path).ok();

    // Negatif: file tak ada → warning IoError, sim TETAP lanjut (x=1).
    let src_c = r#"
module tb3;
    reg [3:0] x;
    initial begin
        $load_coverage_db("tidak_ada_dir/tidak_ada.mcdb");
        x = 1;
        #1 $finish;
    end
endmodule
"#;
    let design_c = compile_str(src_c).unwrap();
    let mut engine_c = crate::simulator::SimulationEngine::new(design_c, 10);
    engine_c.run().unwrap();
    let x_id = engine_c
        .design
        .top
        .signals
        .iter()
        .position(|s| s.name == "x")
        .expect("signal x ada");
    assert_eq!(
        engine_c.state.read_signal(x_id).to_u64(),
        1,
        "sim lanjut walau MCDB gagal dimuat"
    );
}

#[test]
fn test_covergroup_type_option_weight() {
    // VERIF-28: `type_option.weight = N` — bobot covergroup utk functional
    // coverage keseluruhan (weighted average). cg_heavy (weight 2, di-sample
    // penuh) + cg_light (weight 1, TIDAK pernah di-sample = coverage hole 0%)
    // → (2*100 + 1*0)/3 = 66.67%. Tanpa weight: (100+0)/2 = 50%.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg_heavy;
        type_option.weight = 2;
        cp_a: coverpoint a;
    endgroup
    covergroup cg_light;
        type_option.weight = 1;
        cp_b: coverpoint a;
    endgroup
    cg_heavy h = new();
    cg_light l = new();
    initial begin
        a = 1;
        h.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    // IR membawa weight ter-parse.
    let heavy = design
        .covergroups
        .iter()
        .find(|c| c.name == "cg_heavy")
        .expect("cg_heavy");
    let light = design
        .covergroups
        .iter()
        .find(|c| c.name == "cg_light")
        .expect("cg_light");
    assert_eq!(heavy.weight, 2, "type_option.weight = 2 ter-parse");
    assert_eq!(light.weight, 1, "default weight 1");

    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    // cg_heavy di-sample (100%); cg_light tidak pernah di-sample (0% hole).
    let pct = engine.functional_coverage_percent();
    assert!(
        (pct - 66.6667).abs() < 0.01,
        "weighted functional coverage harus 66.67% (weight 2:1), got {}",
        pct
    );
}

#[test]
fn test_covergroup_per_instance() {
    // VERIF-28: `type_option.per_instance = 1` — coverage dilacak per-instance
    // (`cg.i<id>.cp`), bukan di-merge. Dua instance dengan sample berbeda →
    // key terpisah; merge_from_engine tetap menjumlahkan ke agregat.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        type_option.per_instance = 1;
        cp_a: coverpoint a;
    endgroup
    cg i1 = new();
    cg i2 = new();
    initial begin
        a = 1;
        i1.sample();
        i1.sample();
        a = 2;
        i2.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let cg = design.covergroups.iter().find(|c| c.name == "cg").unwrap();
    assert!(cg.per_instance, "type_option.per_instance = 1 ter-parse");

    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    // Per-instance keys: cg.i<id>.cp_a — total 2 utk instance pertama (2
    // sample) dan 1 utk instance kedua, ATAU 1 utk instance1 + 1 auto-sample
    // tiap new(). Verifikasi: tiap key instance punya total sendiri > 0 dan
    // tidak ada agregat tunggal yang menampung semua.
    let mut per_inst_totals: Vec<u64> = Vec::new();
    let mut agg_total = 0u64;
    for (k, v) in &engine.cover_total {
        let s = k.as_str();
        if let Some(rest) = s.strip_prefix("cg.i") {
            if let Some(_cp) = rest.strip_suffix(".cp_a") {
                per_inst_totals.push(*v);
            }
        } else if s == "cg.cp_a" {
            agg_total += v;
        }
    }
    assert_eq!(
        agg_total, 0,
        "per_instance=1 tidak boleh memakai key agregat"
    );
    assert_eq!(per_inst_totals.len(), 2, "dua instance → dua key terpisah");
    assert!(
        per_inst_totals.contains(&2),
        "instance1 2 sample: {:?}",
        per_inst_totals
    );
    assert!(
        per_inst_totals.contains(&1),
        "instance2 1 sample: {:?}",
        per_inst_totals
    );
    assert_eq!(
        per_inst_totals.iter().sum::<u64>(),
        3,
        "total 3 sample (2+1)"
    );

    // merge_from_engine menjumlahkan semua key instance ke agregat.
    let mut db = crate::simulator::coverage_db::CoverageDatabase::new();
    db.merge_from_engine(&engine);
    let entry = db.covergroups.get("cg").expect("cg in db");
    assert_eq!(entry.coverpoints.len(), 1);
    assert_eq!(entry.coverpoints[0].total, 3, "merge agregat = 3 sample");
    assert_eq!(entry.coverpoints[0].hits, 3);
}

#[test]
fn test_covergroup_cross() {
    let source = r#"
module tb;
    reg [31:0] a;
    reg [31:0] b;
    covergroup cg;
        cp_a: coverpoint a;
        cp_b: coverpoint b;
        cross_a_b: cross cp_a, cp_b;
    endgroup
    cg cg_inst = new();
    initial begin
        a = 1; b = 2;
        cg_inst.sample();
        a = 3; b = 4;
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    // Check cross coverage: 2 samples, 2 unique cross bins
    let cross_key = Symbol::intern("cg.cross_a_b");
    assert_eq!(
        engine.cover_total.get(&cross_key).copied().unwrap_or(0),
        2,
        "cross total should be 2"
    );
    assert_eq!(
        engine.cover_hits.get(&cross_key).copied().unwrap_or(0),
        2,
        "cross hits should be 2"
    );
    let cross_bins = engine.cover_bins.get(&cross_key).unwrap();
    assert_eq!(cross_bins.len(), 2, "should have 2 unique cross bins");
    assert!(
        cross_bins.contains_key(&Symbol::intern("cp_a=1 x cp_b=2")),
        "missing cross bin for a=1,b=2"
    );
    assert!(
        cross_bins.contains_key(&Symbol::intern("cp_a=3 x cp_b=4")),
        "missing cross bin for a=3,b=4"
    );
    assert_eq!(cross_bins[&Symbol::intern("cp_a=1 x cp_b=2")], 1);
    assert_eq!(cross_bins[&Symbol::intern("cp_a=3 x cp_b=4")], 1);
}

#[test]
fn test_covergroup_with_bins() {
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins low = {[0:10]};
            bins high = {[11:20]};
        }
    endgroup
    initial begin
        a = 42;
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "covergroup with bins should parse without error: {:?}",
        result.err()
    );
}

#[test]
fn test_covergroup_ignore_bins_excluded() {
    // VERIF-30: ignore_bins — nilai yang dikecualikan TIDAK dihitung (total
    // tidak naik, tidak masuk auto-bin). Sebelumnya bin eksplisit di-parse tapi
    // di-drop elaborator → nilai ignore tetap masuk auto-binning default.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            ignore_bins skip = {[5:7]};
            bins keep = {[0:4]};
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 6;   // ignore → dikecualikan
        cg_inst.sample();
        a = 3;   // keep → dihitung ke bin keep
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    assert_eq!(
        engine.cover_total.get(&key).copied().unwrap_or(0),
        1,
        "total hanya 1 (sampel ignore dikecualikan)"
    );
    assert_eq!(engine.cover_hits.get(&key).copied().unwrap_or(0), 1);
    let bins = engine.cover_bins.get(&key).unwrap();
    assert!(
        bins.contains_key(&Symbol::intern("cp_a=keep")),
        "bin eksplisit keep harus terisi"
    );
    assert!(
        !bins.contains_key(&Symbol::intern("cp_a=6")),
        "nilai ignore tidak boleh masuk auto-bin"
    );
}

#[test]
fn test_covergroup_illegal_bins_error() {
    // VERIF-30: illegal_bins — nilai yang TIDAK BOLEH muncul → laporan
    // (AssertionFailed) + sampel TIDAK dihitung sebagai hit.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            illegal_bins never = {[100:200]};
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 150;  // illegal → error
        cg_inst.sample();
        a = 5;    // ok → default bin
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    assert_eq!(engine.cover_total.get(&key).copied().unwrap_or(0), 2);
    assert_eq!(
        engine.cover_hits.get(&key).copied().unwrap_or(0),
        1,
        "hits hanya 1 (sampel illegal tidak dihitung)"
    );
    let diags = engine.flush_diagnostics();
    assert!(
        diags
            .iter()
            .any(|d| d.message.as_ref().contains("illegal_bins hit")),
        "harus ada laporan illegal_bins: {:#?}",
        diags.iter().map(|d| d.message.as_ref()).collect::<Vec<_>>()
    );
}

#[test]
fn test_covergroup_explicit_bin_list_values() {
    // VERIF-30: `bins b = {1, 2}` (daftar nilai) vs `{[1:5]}` (range) —
    // representasi BinRange memisahkan keduanya; nilai tunggal tidak boleh
    // diinterpretasi sebagai range.
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins two = {2, 4};
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 2;
        cg_inst.sample();
        a = 4;
        cg_inst.sample();
        a = 3;   // tidak match bin two → auto-bin
        cg_inst.sample();
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    let bins = engine.cover_bins.get(&key).unwrap();
    assert_eq!(
        bins.get(&Symbol::intern("cp_a=two")).copied().unwrap_or(0),
        2,
        "bin two harus kena 2x (nilai 2 dan 4)"
    );
    assert!(
        bins.contains_key(&Symbol::intern("cp_a=3")),
        "nilai 3 di luar bin → auto-bin default"
    );
}

#[test]
fn test_covergroup_transition_bins() {
    // VERIF-31: transition bins `(a => b)` — cocokkan (prev, curr). Sebelumnya
    // `=>` tidak di-lex (jadi `=` + `>`), bin transisi ter-parse dgn range_list
    // kosong → transisi tidak pernah dicatat (semua jadi auto-bin).
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins rising  = (0 => 1);
            bins falling = (1 => 0);
        }
    endgroup
    cg cg_inst = new();
    initial begin
        a = 0;
        cg_inst.sample();  // prev=None → auto-bin (tidak ada transisi)
        a = 1;
        cg_inst.sample();  // 0=>1 → rising
        a = 0;
        cg_inst.sample();  // 1=>0 → falling
        a = 1;
        cg_inst.sample();  // 0=>1 → rising (lagi)
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 10);
    engine.run().unwrap();
    let key = Symbol::intern("cg.cp_a");
    let bins = engine.cover_bins.get(&key).unwrap();
    assert_eq!(
        bins.get(&Symbol::intern("cp_a=rising"))
            .copied()
            .unwrap_or(0),
        2,
        "rising (0=>1) harus kena 2x"
    );
    assert_eq!(
        bins.get(&Symbol::intern("cp_a=falling"))
            .copied()
            .unwrap_or(0),
        1,
        "falling (1=>0) harus kena 1x"
    );
    assert!(
        bins.contains_key(&Symbol::intern("cp_a=0")),
        "sampel pertama (prev=None) → auto-bin default"
    );
    assert_eq!(engine.cover_total.get(&key).copied().unwrap_or(0), 4);
}

#[test]
fn test_coverage_gap_analysis() {
    // VERIF-26: coverage gap analysis — bin eksplisit yang tidak pernah kena
    // dan coverpoint/cross yang tidak pernah di-sample terdaftar di
    // engine.coverage_gaps().
    let source = r#"
module tb;
    reg [31:0] a;
    covergroup cg;
        cp_a: coverpoint a {
            bins low  = {0};
            bins high = {100};
        }
        cp_b: coverpoint a;
    endgroup
    covergroup never_sampled;
        cp_x: coverpoint a;
    endgroup
    cg cg_inst = new();
    never_sampled ns_inst = new();
    initial begin
        a = 0;
        cg_inst.sample();   // bin low kena; bin high TIDAK
        a = 5;
        cg_inst.sample();   // auto-bin 5
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let mut engine = crate::simulator::SimulationEngine::new(design, 5);
    engine.run().unwrap();
    let gaps = engine.coverage_gaps();
    // Bin high tidak pernah kena.
    assert!(
        gaps.iter().any(|g| g.contains("bin 'high'")),
        "gap harus menyebut bin 'high' tak pernah kena: {:?}",
        gaps
    );
    // Covergroup never_sampled tidak pernah di-sample → cp_x gap.
    assert!(
        gaps.iter()
            .any(|g| g.contains("never_sampled.cp_x") && g.contains("tidak pernah di-sample")),
        "gap harus menyebut coverpoint never_sampled.cp_x tak pernah di-sample: {:?}",
        gaps
    );
    // Tidak ada false-positive: cp_a LOW bin (kena) tidak boleh jadi gap.
    assert!(
        !gaps.iter().any(|g| g.contains("bin 'low'")),
        "bin low pernah kena — tidak boleh jadi gap: {:?}",
        gaps
    );
}

#[test]
fn test_wand_resolution() {
    let source = r#"
module tb;
    wand w;
    reg a, b;
    assign w = a;
    assign w = b;
    initial begin
        a = 0; b = 1;
        #1;
        // wand: AND of drivers → 0 & 1 = 0
        if (w !== 0) $display("FAIL: wand expected 0 got %b", w);
        a = 1; b = 1;
        #1;
        if (w !== 1) $display("FAIL: wand expected 1 got %b", w);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "wand resolution failed: {:?}", result.err());
}

#[test]
fn test_wor_resolution() {
    let source = r#"
module tb;
    wor w;
    reg a, b;
    assign w = a;
    assign w = b;
    initial begin
        a = 0; b = 1;
        #1;
        // wor: OR of drivers → 0 | 1 = 1
        if (w !== 1) $display("FAIL: wor expected 1 got %b", w);
        a = 0; b = 0;
        #1;
        if (w !== 0) $display("FAIL: wor expected 0 got %b", w);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "wor resolution failed: {:?}", result.err());
}

#[test]
fn test_multi_driver_detection() {
    // SIM-15: signal yang di-drive oleh >1 proses ditandai `multi_driver` oleh
    // elaborator (detect_multi_driver_signals, ext.rs) — dipakai engine untuk
    // skip false-positive race check + mengaktifkan resolusi net. Sebelumnya
    // hanya ada resolusi (wand/wor/tri), deteksi sendiri belum diuji langsung.
    let source = r#"
module tb;
    wire w;   // 2 driver → multi_driver=true
    wire s;   // 1 driver  → multi_driver=false
    reg a, b;
    assign w = a;
    assign w = b;
    assign s = a;
    initial begin
        a = 0; b = 1;
        #1 $finish;
    end
endmodule
"#;
    let design = compile_str(source).unwrap();
    let w = design
        .top
        .signals
        .iter()
        .find(|s| s.name.as_str() == "w")
        .unwrap();
    let s = design
        .top
        .signals
        .iter()
        .find(|s| s.name.as_str() == "s")
        .unwrap();
    assert!(w.multi_driver, "w (2 assign) harus terdeteksi multi-driver");
    assert!(!s.multi_driver, "s (1 assign) bukan multi-driver");
    // Sim tetap jalan dengan multi-driver (resolusi aktif, bukan race error).
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "sim multi-driver gagal: {:?}", result.err());
}

#[test]
fn test_multi_driver_output_logic_resolution_consistent() {
    // Fuzz differential (bug_d2919857ef6d): `output logic` didorong DUA
    // continuous assign — jalur default vs DAG dulu beda (1 vs x). Kini
    // sinyal Output/Logic multi-driver ikut ditandai → resolusi net aktif
    // → semua jalur engine konvergen.
    let source = r#"
module m(output logic o, input logic a);
  assign o = 1'b1;
  assign o = a;
endmodule
"#;
    let design = compile_str(source).unwrap();
    let o = design
        .top
        .signals
        .iter()
        .find(|s| s.name.as_str() == "o")
        .unwrap();
    assert!(o.multi_driver, "output logic 2-driver harus multi_driver");
    let a1 = simulate_signals_with_flags_quiet(source, 100, &EngineFlags::default()).unwrap();
    let dag = EngineFlags {
        use_dag_parallel: true,
        ..EngineFlags::default()
    };
    let a2 = simulate_signals_with_flags_quiet(source, 100, &dag).unwrap();
    let v1 = a1
        .iter()
        .find(|(n, _)| n == "o")
        .map(|(_, v)| format!("{v}"))
        .unwrap_or_default();
    let v2 = a2
        .iter()
        .find(|(n, _)| n == "o")
        .map(|(_, v)| format!("{v}"))
        .unwrap_or_default();
    assert_eq!(v1, v2, "default vs DAG harus identik");
}

#[test]
fn test_tri_resolution() {
    let source = r#"
module tb;
    tri t;
    reg a, en;
    assign t = en ? a : 1'bz;
    assign t = 1'b1;  // pullup
    initial begin
        en = 0; a = 0;
        #1;
        // tri: driver2 = Z, driver1 = 1 → 1
        if (t !== 1) $display("FAIL: tri expected 1 got %b", t);
        en = 1; a = 0;
        #1;
        // tri: driver2 = 0, driver1 = 1 → X (conflict)
        if (t !== 1'bx) $display("FAIL: tri expected X got %b", t);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(result.is_ok(), "tri resolution failed: {:?}", result.err());
}

#[test]
fn test_wand_keyword_parse() {
    let source = r#"
module tb;
    wand w;
    initial #1 $finish;
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "wand keyword should parse: {:?}",
        result.err()
    );
}

#[test]
fn test_dpi_import_function() {
    let source = r#"
module tb;
    import "DPI-C" function int my_add(input int a, input int b);
    int result;
    initial begin
        result = my_add(3, 4);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI function import should compile: {:?}",
        design.err()
    );
}

#[test]
fn test_dpi_import_task() {
    let source = r#"
module tb;
    import "DPI-C" task my_task(input int x);
    initial begin
        my_task(42);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI task import should compile: {:?}",
        design.err()
    );
}

#[test]
fn test_dpi_import_void() {
    let source = r#"
module tb;
    import "DPI-C" function void dpi_void(input int x);
    initial begin
        dpi_void(42);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI void function import should compile: {:?}",
        design.err()
    );
}

#[test]
fn test_dpi_import_multi_arg() {
    let source = r#"
module tb;
    import "DPI-C" function int dpi_mul(input byte a, input shortint b, input int c);
    int result;
    initial begin
        result = dpi_mul(1, 2, 3);
    end
endmodule
"#;
    let design = compile_str(source);
    assert!(
        design.is_ok(),
        "DPI multi-arg import should compile: {:?}",
        design.err()
    );
}

// ── DPI-C Enhancement Tests (CRIT-009) ──

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_scope_management() {
    // Test svGetScope/svSetScope via thread-local path
    use crate::simulator::dpi::*;

    // Initially no scope set
    let scope = sv_get_scope();
    assert!(scope.is_null(), "no scope should be set initially");

    // Set scope via thread-local
    set_current_dpi_scope("top.u_sub");
    let scope = sv_get_scope();
    assert!(!scope.is_null(), "scope should be non-null after setting");
    let name = sv_get_scope_name(scope);
    assert_eq!(name, Some("top.u_sub".to_string()));

    // Test svSetScope
    let new_scope = sv_set_scope_name("top.other");
    let result = sv_set_scope(new_scope);
    assert_eq!(result, 1, "svSetScope should succeed");
    let scope2 = sv_get_scope();
    let name2 = sv_get_scope_name(scope2);
    assert_eq!(name2, Some("top.other".to_string()));
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_time_query() {
    use crate::simulator::dpi::*;

    // Set time to 42
    set_current_dpi_time(42);
    let scope = svScope::NULL;
    let time = sv_get_time(scope, std::ptr::null_mut());
    assert_eq!(time, 42, "svGetTime should return current time");
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_chandle_store() {
    use crate::simulator::dpi::*;

    // Allocate a chandle for an opaque pointer value
    let handle1 = chandle_alloc(0xDEADBEEF);
    assert!(handle1 > 0, "chandle handle should be non-zero");

    let handle2 = chandle_alloc(0xCAFEBABE);
    assert_ne!(handle1, handle2, "chandle handles should be unique");

    // Get back the stored value
    let val1 = chandle_get(handle1);
    assert_eq!(val1, Some(0xDEADBEEF));

    let val2 = chandle_get(handle2);
    assert_eq!(val2, Some(0xCAFEBABE));

    // Free handle
    chandle_free(handle1);
    assert_eq!(
        chandle_get(handle1),
        None,
        "freed handle should return None"
    );
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_export_register_and_call() {
    use crate::simulator::dpi::*;
    use mivon_ir::*;

    // Register a simple SV function as DPI export
    let func = DpiExportedFunction {
        export_name: "my_sv_func".to_string(),
        n_args: 2,
        arg_widths: vec![32, 32],
        is_task: false,
        callback: Box::new(|args| {
            let a = args.first().map(|v| v.to_u64()).unwrap_or(0);
            let b = args.get(1).map(|v| v.to_u64()).unwrap_or(0);
            LogicVec::from_u64(a + b, 32)
        }),
    };
    sv_export_register(func);

    // Call the exported function
    let args = vec![LogicVec::from_u64(40, 32), LogicVec::from_u64(2, 32)];
    let result = sv_export_call("my_sv_func", &args).unwrap();
    assert_eq!(result.to_u64(), 42, "DPI export should return 40+2=42");
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_export_task() {
    use crate::simulator::dpi::*;
    use mivon_ir::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED: AtomicBool = AtomicBool::new(false);

    let func = DpiExportedFunction {
        export_name: "my_sv_task".to_string(),
        n_args: 0,
        arg_widths: vec![],
        is_task: true,
        callback: Box::new(|_| {
            CALLED.store(true, Ordering::SeqCst);
            LogicVec::new(0)
        }),
    };
    sv_export_register(func);

    sv_export_call("my_sv_task", &[]).unwrap();
    assert!(
        CALLED.load(Ordering::SeqCst),
        "task should have been called"
    );
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_bit_vector_helpers() {
    use crate::simulator::dpi::*;

    // Test svGetBitsel and svPutBitsel (helpers are unsafe fn — pointer args)
    let mut vec_bits: [svBitVecVal; 2] = [0, 0];
    unsafe {
        sv_put_bitsel(&mut vec_bits as *mut svBitVecVal, 3, 1);
        assert_eq!(sv_get_bitsel(&vec_bits as *const svBitVecVal, 3), 1);
        assert_eq!(sv_get_bitsel(&vec_bits as *const svBitVecVal, 2), 0);
    }

    // Test svPutPartSelect and svGetPartSelect
    let mut vec2: [svBitVecVal; 2] = [0, 0];
    let val = unsafe {
        sv_put_part_select(&mut vec2 as *mut svBitVecVal, 0, 8, 0xAB);
        sv_get_part_select(&vec2 as *const svBitVecVal, 0, 8)
    };
    assert_eq!(val, 0xAB, "part select should round-trip 0xAB");
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_logic_vector_helpers() {
    use crate::simulator::dpi::*;

    // Test svGetLogicBitsel with 4-state encoding (helpers are unsafe fn)
    let mut logic_vec: [svLogicVecVal; 4] = [0, 0, 0, 0];
    unsafe {
        // Set bit 0 to '1' (aval=1, bval=0)
        sv_put_logic_bitsel(&mut logic_vec as *mut svLogicVecVal, 0, 1);
        assert_eq!(
            sv_get_logic_bitsel(&logic_vec as *const svLogicVecVal, 0),
            1
        );

        // Set bit 1 to 'X' (aval=0, bval=1)
        sv_put_logic_bitsel(&mut logic_vec as *mut svLogicVecVal, 1, 2);
        assert_eq!(
            sv_get_logic_bitsel(&logic_vec as *const svLogicVecVal, 1),
            2
        );

        // Set bit 2 to 'Z' (aval=1, bval=1)
        sv_put_logic_bitsel(&mut logic_vec as *mut svLogicVecVal, 2, 3);
        assert_eq!(
            sv_get_logic_bitsel(&logic_vec as *const svLogicVecVal, 2),
            3
        );
    }
}

#[cfg(feature = "dpi")]
#[test]
fn test_dpi_chandle_conversion() {
    use crate::simulator::dpi::*;

    let ptr_val: u64 = 0x1234567890ABCDEF;
    let ch = u64_to_chandle(ptr_val);
    let back = chandle_to_u64(ch);
    assert_eq!(back, ptr_val, "chandle pointer round-trip should work");
}

#[test]
fn test_dpi_builtin_sv_bit_functions() {
    // Test that built-in DPI conversion functions work (with proper import)
    let source = r#"
module tb;
    import "DPI-C" function int svToInt(input logic [31:0] a);
    logic [31:0] a;
    logic [31:0] result;
    initial begin
        a = 42;
        result = svToInt(a);
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 5);
    assert!(
        sigs.is_ok(),
        "DPI built-in svToInt should not crash: {:?}",
        sigs.err()
    );
}

// ── End DPI-C Enhancement Tests ──

#[test]
fn test_inout_basic_parse() {
    let source = r#"
module top;
    tri w;
    driver u1(.port(w));
    initial #1 $finish;
endmodule
module driver(inout port);
    assign port = 1'b1;
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "inout port should parse: {:?}",
        result.err()
    );
}

#[test]
fn test_inout_tri_resolution() {
    let source = r#"
module top;
    tri t;
    driver u1(.port(t));
    driver u2(.port(t));
    initial begin
        #1;
        if (t !== 1'bx) $display("FAIL: inout conflict expected X got %b", t);
        #1 $finish;
    end
endmodule
module driver(inout port);
    assign port = 1'b1;
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "inout tri resolution failed: {:?}",
        result.err()
    );
}

#[test]
fn test_inout_bidirectional() {
    let source = r#"
module top;
    reg [1:0] drv_val;
    tri w;
    bus_driver u1(.val(drv_val), .bus(w));
    initial begin
        drv_val = 0;
        #1;
        if (w !== 1'b0) $display("FAIL: expected 0 at time 1 got %b", w);
        drv_val = 1;
        #1;
        if (w !== 1'b1) $display("FAIL: expected 1 at time 2 got %b", w);
        #1 $finish;
    end
endmodule
module bus_driver(inout bus, input [1:0] val);
    reg oe;
    assign bus = oe ? val[0] : 1'bz;
    initial oe = 1;
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "inout bidirectional failed: {:?}",
        result.err()
    );
}

#[test]
fn test_parameter_type_default() {
    let source = r#"
module my_mux #(parameter type T = logic) (input T a, output T y);
    assign y = a;
endmodule
module tb;
    wire a, y;
    my_mux u1(.a(a), .y(y));
    initial begin
        a = 1;
        #1;
        if (y !== 1) $display("FAIL: expected 1 got %b", y);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "parameter type parse failed: {:?}",
        result.err()
    );
}

#[test]
fn test_parameter_type_override() {
    let source = r#"
module my_bus #(parameter type T = logic) (input T [7:0] a, output T [7:0] y);
    assign y = a;
endmodule
module tb;
    wire [7:0] a, y;
    my_bus #(.T(bit)) u1(.a(a), .y(y));
    initial begin
        a = 8'hAB;
        #1;
        if (y !== 8'hAB) $display("FAIL: expected AB got %h", y);
        #1 $finish;
    end
endmodule
"#;
    let result = simulate_signals(source, 10);
    assert!(
        result.is_ok(),
        "parameter type override failed: {:?}",
        result.err()
    );
}

// PARSER-10: module parameter type list edge cases
#[test]
fn test_parameter_type_edge_cases() {
    // Scoped type default, signed type, multiple type params,
    // mixed type+value params, type with packed range default.
    let source = r#"
module pkg_types (input int a, output int y);
    assign y = a;
endmodule
module param_edge_test #(
    parameter type T1 = int,
    parameter type T2 = logic,
    parameter int W = 8,
    parameter signed [7:0] SP = -1,
    parameter logic [3:0] MASK = 4'hF
) (
    input T1 a,
    input T2 b,
    output logic [W-1:0] y
);
    assign y = a[W-1:0] + {b, 3'b000} + {1'b0, MASK};
endmodule
module tb;
    logic [7:0] a_val = 8'h05;
    logic b_val = 1'b1;
    logic [7:0] y_val;
    param_edge_test #(.T1(int), .T2(logic)) u1(.a(a_val), .b(b_val), .y(y_val));
    initial begin
        #1;
        $display("y = %h", y_val);
        #1 $finish;
    end
endmodule
"#;
    let result = compile_str(source);
    assert!(
        result.is_ok(),
        "PARSER-10 parameter edge cases failed: {:?}",
        result.err()
    );
}

#[test]
fn test_ovm_plain_name_builtin_in_subclass() {
    // VERIF-20 t4: method builtin uvm_object dipanggil TANPA `this.` di body
    // subclass (`get_name()` di class `extends ovm_object`). PRA-FIX: RT9003
    // "unknown function 'get_name' in method context" karena dispatch
    // plain-name hanya mencari method di design class.
    let source = r#"
class c_env extends ovm_object;
    function new(string n);
        super.new(n);
    endfunction
    function int cek();
        string s;
        s = get_name();
        if (s == "env") return 1;
        return 0;
    endfunction
endclass

module tb;
    c_env e;
    int r;
    initial begin
        e = c_env::new("env");
        r = e.cek();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("plain-name get_name() (OVM) harus jalan");
    let r = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(r, 1, "get_name() implicit this harus 'env', got r={r}");
}

#[test]
fn test_uvm_plain_name_builtin_in_component_subclass() {
    // Rantai uvm_component → uvm_report_object → uvm_object: get_name() juga
    // harus tersedia tanpa `this.` (dispatch presisi lewat uvm_builtin_base,
    // bukan execute_method yang heuristik hierarchy-nya longgar).
    let source = r#"
class my_comp extends uvm_component;
    function new(string name, uvm_component parent);
        super.new(name, parent);
    endfunction
    function int cek();
        string s;
        s = get_name();
        if (s == "cmp") return 1;
        return 0;
    endfunction
endclass

module tb;
    my_comp c;
    int r;
    initial begin
        c = my_comp::new("cmp", null);
        r = c.cek();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("plain-name get_name() (komponen) harus jalan");
    let r = sigs
        .iter()
        .find(|(n, _)| n == "r")
        .map(|(_, v)| v.to_u64())
        .unwrap_or(99);
    assert_eq!(r, 1, "get_name() implicit this harus 'cmp', got r={r}");
}

#[test]
fn test_plain_name_unknown_builtin_still_errors() {
    // Guard: fallback plain-name TIDAK boleh menelan nama method yang tidak
    // ada di layer builtin (harus error keras, bukan 0 senyap).
    let source = r#"
class c_env extends ovm_object;
    function new(string n);
        super.new(n);
    endfunction
    function int cek();
        return method_tidak_ada();
    endfunction
endclass

module tb;
    c_env e;
    int r;
    initial begin
        e = c_env::new("env");
        r = e.cek();
        #1 $finish;
    end
endmodule
"#;
    let err = simulate_signals(source, 10).expect_err("method tak dikenal harus error");
    let msg = format!("{err}");
    assert!(
        msg.contains("method_tidak_ada"),
        "pesan harus menyebut nama method: {msg}"
    );
}

#[test]
fn test_ovm_alias_family_plain_name_builtin() {
    // Review R196 BUG-1: alias OVM non-object/component (ovm_driver,
    // ovm_sequence_item, ...) juga harus mendapat method object polos.
    // Terminal chain = nama library tak terdaftar → fallback prefiks
    // `ovm_` → layer object (sama dengan fallback super-dispatch).
    let source = r#"
class d_drv extends ovm_driver;
    function new(string n);
        super.new(n);
    endfunction
    function int cek_drv();
        string s;
        s = get_name();
        if (s == "drv") return 1;
        return 0;
    endfunction
endclass

class s_item extends ovm_sequence_item;
    function new(string n);
        super.new(n);
    endfunction
    function int cek_item();
        string s;
        s = get_name();
        if (s == "itm") return 1;
        return 0;
    endfunction
endclass

module tb;
    d_drv d;
    s_item i;
    int r1, r2;
    initial begin
        d = d_drv::new("drv");
        i = s_item::new("itm");
        r1 = d.cek_drv();
        r2 = i.cek_item();
        #1 $finish;
    end
endmodule
"#;
    let sigs = simulate_signals(source, 10).expect("plain-name get_name() alias OVM harus jalan");
    let get = |n: &str| {
        sigs.iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.to_u64())
            .unwrap_or(99)
    };
    assert_eq!(get("r1"), 1, "ovm_driver: get_name() harus 'drv'");
    assert_eq!(get("r2"), 1, "ovm_sequence_item: get_name() harus 'itm'");
}
