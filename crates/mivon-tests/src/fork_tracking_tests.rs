//! PERF-20: ForkGroup tracking — regression akuntansi fork (stale marker
//! terverifikasi, ROUND 87).
//!
//! Klaim audit: "ForkGroup remaining decrement O(fork_branches) per
//! completion → fix atomic counter". FAKTA di kode sekarang:
//! - `fork_decrement(fid)` (core.rs) = indeks LANGSUNG `fork_groups[fid]`
//!   — O(1), tanpa scan; `fid` dibawa `Continuation.fork_id`.
//! - `alloc_fork_group` O(1): free-list `fork_free.pop()` (reuse slot
//!   retired) atau push.
//! - Scan `fork_groups` yang ada (`active_fork_groups_for_current_process`,
//!   `disable_fork_by_label`) hanya dipanggil per-STATEMENT
//!   `wait fork`/`disable fork`/`disable <label>` (konstruk langka), bukan
//!   per-completion. (`fork_finish` → `check_wait_forks` memang scan
//!   `pending_wait_forks` per-completion — tapi bukan O(fork_branches) dan
//!   biasanya kosong.) Scheduler single-threaded → "atomic counter" tidak
//!   relevan (`remaining: usize` sudah jadi counter).
//!
//! Test di bawah mengunci SEMANTIK tracking yang jadi inti klaim:
//! counter akurat (sekali decrement per branch, continuation fire sekali),
//! isolasi antar group/proses (indeks fid benar), reuse slot anti-leak.

use super::*;

fn sim(source: &str, max_time: u64) -> Vec<(String, mivon_ir::LogicVec)> {
    simulate_signals(source, max_time)
        .unwrap_or_else(|e| panic!("sim gagal: {e}\n{source}"))
}

fn get_u64(sigs: &[(String, mivon_ir::LogicVec)], name: &str) -> u64 {
    sigs.iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("signal tidak ada: {name}"))
        .1
        .to_u64()
}

/// Reuse slot ForkGroup di dalam loop (alloc → retire → reuse via
/// `fork_free`): continuation fire TEPAT SEKALI per iterasi (flag `fired`
/// harus di-reset saat slot dipakai ulang — double-fire = bug akuntansi),
/// branch execution sekali (counter `remaining` tak kehilangan branch).
/// Catatan: jumlah slot fork_groups tak terobservasi dari SV — yang dikunci
/// di sini semantic fired-reset + akuntansi, bukan jumlah slot.
#[test]
fn fork_slot_reuse_in_loop_continuation_once() {
    let source = r#"
module tb;
    int cnt;
    int fires;
    initial begin
        cnt = 0; fires = 0;
        for (int i = 0; i < 50; i++) begin
            fork
                #1 cnt = cnt + 1;
            join
            fires = fires + 1;
        end
    end
    initial #100 $finish;
endmodule
"#;
    let sigs = sim(source, 150);
    assert_eq!(get_u64(&sigs, "cnt"), 50, "branch fire sekali per iterasi");
    assert_eq!(
        get_u64(&sigs, "fires"),
        50,
        "post-join continuation fire sekali per iterasi (slot reuse reset fired)"
    );
}

/// Fan-out lebar: 64 branch dalam SATU fork join — `remaining` akurat
/// (tak ada branch terlewat/double-decrement → continuation premature).
#[test]
fn fork_wide_fanout_all_branches_complete() {
    let mut branches = String::new();
    for i in 0..64u32 {
        branches.push_str(&format!("            #{i} got[{i}] = 1'b1;\n"));
    }
    let source = format!(
        r#"
module tb;
    logic [63:0] got;
    int done;
    int done_t;
    initial begin
        got = 64'h0; done = 0; done_t = 0;
        fork
{branches}        join
        done_t = $time;
        done = 1;
    end
    initial #200 $finish;
endmodule
"#
    );
    let sigs = sim(&source, 250);
    assert_eq!(
        get_u64(&sigs, "got"),
        u64::MAX,
        "semua 64 branch harus menulis bit-nya (remaining tak kehilangan branch)"
    );
    assert_eq!(
        get_u64(&sigs, "done_t"),
        63,
        "continuation join fire di t=63 (setelah branch terakhir) — bukan premature"
    );
    assert_eq!(
        get_u64(&sigs, "done"),
        1,
        "continuation join fire SETELAH semua 64 branch (bukan premature)"
    );
}

/// Fork bersarang: decrement group DALAM tak boleh menyelesaikan group
/// LUAR (fid terpisah, indeks langsung). Bila tercampur, outer join
/// selesai premature (t=4 alih-alih t=6).
#[test]
fn fork_nested_groups_isolated() {
    let source = r#"
module tb;
    int inner_done;
    int outer_time;
    logic ia, ib, ob;
    initial begin
        inner_done = 0; outer_time = 0;
        fork
            begin
                fork
                    #2 ia = 1'b1;
                    #4 ib = 1'b1;
                join
                inner_done = 1;
            end
            #6 ob = 1'b1;
        join
        outer_time = $time;
    end
    initial #20 $finish;
endmodule
"#;
    let sigs = sim(source, 30);
    assert_eq!(get_u64(&sigs, "inner_done"), 1, "group dalam selesai");
    assert_eq!(
        get_u64(&sigs, "outer_time"),
        6,
        "outer join di t=6 (max cabang) — decrement group dalam tak menyelesaikan outer"
    );
    assert_eq!(get_u64(&sigs, "ob"), 1, "cabang outer kedua jalan");
}

/// Filter spawner (`active_fork_groups_for_current_process` — satu-satunya
/// scan Vec, per-statement `wait fork`): `wait fork` proses A hanya
/// Filter spawner (`active_fork_groups_for_current_process` — scan
/// `fork_groups`, dipanggil per-statement `wait fork`): `wait fork` proses
/// A hanya menunggu group A. `#1` SEBELUM `wait fork` A penting: pada t=1
/// group B SUDAH ada (B dieksekusi di t=0) — tanpa jeda ini, A query saat
/// baru group A yang ada → mutasi "hapus predicate spawner" lolos (vacuous).
/// Dgn jeda: filter rusak → A ikut menunggu B → a_done_t = 15 ≠ 5.
#[test]
fn wait_fork_spawner_filter_isolates_processes() {
    let source = r#"
module tb;
    int a_done_t;
    int b_done_t;
    int a1, a2, y1;
    initial begin
        fork
            #5 a1 = 1;
            #5 a2 = 1;
        join_none
        #1
        wait fork;
        a_done_t = $time;
    end
    initial begin
        fork
            #15 y1 = 1;
        join_none
        wait fork;
        b_done_t = $time;
    end
    initial #30 $finish;
endmodule
"#;
    let sigs = sim(source, 40);
    assert_eq!(
        get_u64(&sigs, "a_done_t"),
        5,
        "wait fork A hanya menunggu group A (t=5, bukan t=15 milik B)"
    );
    assert_eq!(
        get_u64(&sigs, "b_done_t"),
        15,
        "wait fork B menunggu branch B (t=15)"
    );
}
