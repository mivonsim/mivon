# Buglog mivon-mv — hasil eksplorasi (pencari bug mivon utama)

Pipeline `.mv` → transpile → SV → sim dipakai sebagai oracle untuk menemukan
bug di mivon utama (parser/elaborator/simulator). Status: ✅ fixed / ⏳ open.

## ✅ Fixed

1. **Array decl-init assignment pattern reversed** — `rom [0:3] = '{a,b,c,d}`
   menghasilkan `rom[0]=d` (Concat MSB-first vs storage unpacked elemen-0 di
   bit terendah; decl-init di-fold jadi Const lalu write utuh). Fix: decompose
   Concat → per-elemen assign di decl-init (pola sama dgn statement assign).

2. **VCD array elemen lebar salah** — `rom[i]` di-emit 32-bit `[31:0]` padahal
   elemen 8-bit (VCD header pakai lebar total, bukan width/array_depth).

3. **always_comb array-index sensitivity** — `always_comb val = rom[idx]` tidak
   re-trigger saat `idx` berubah (IR `ArrayIndex` sensitivity hanya collect
   signal dasar, index hilang). Fix: collect index juga.

5. **Signed relational vs literal** — `signed [7:0] s; s=-1; s < 0` → false
   (unsigned 255<0). Fix: SQL relational signed bila SALAH SATU operand signed
   (&& → || di eval/expr.rs); Div/Mod ikut.

6. **Real literal arithmetic NaN** — `1.5 + 2.25` → NaN (evaluator biner hanya
   kenal is_real utk operand signal; literal murni jalur integer). Fix:
   fold real literal+literal di elaborasi (`fold_binary_real`); vs signal
   (variable + literal) sudah jalan via jalur is_real.

7. **Signed Div/Mod vs literal** — `s = -4; sd = s / 2` → 126 (unsigned 252/2).
   Operand sinyal di-cast zero-extend ke 32 oleh konteks (`Cast{32, Signal}`)
   sebelum eval signed → l=252. Fix: clip operand Div/Mod ke lebar ASLI signal
   (signed_raw_operand menembus Cast/Signed wrapper) → -4/2 = -2 (0xFE).

8. **`>>>` arithmetic shift signed** — `signed [7:0] a=-5; a >>> 1` → 125
   (harus -3/0xFD). Sudah tertutup `test_arithmetic_shift_right_signedness`
   (ROUND 36: `s>>>2`=0xE0, `>>` selalu logical, int signed) + ekspresi
   majemuk `(a*2) >>> 2` = -4 — diverifikasi ulang probe `t_p2`:
   `B=-3 C=253` benar. Buglog usang.

4. **Unpacked array MULTI-dimensi (F39)** — parser SV (`mivon-parser`) hanya
   menyimpan SATU dimensi unpacked; `logic [7:0] mat [0:1][0:1]` menjadi
   width 16 / 2 elemen (harus 32 / 4), init `'{'{1,2},'{3,4}}` tak
   ter-decompose → semua elemen 0; index 2-d `mat[i][j]` = bit-select lebar 1.
   Fix lengkap:
   - **AST** (`mivon-ast/src/types.rs`): `DeclVar.extra_unpacked_dims` +
     `Port.extra_unpacked_dims` — `Vec<(Option<Range>, Option<Expr>)>` utk
     dim lanjutan (range ter-resolve / size-expr utk param).
   - **Parser** (`decl.rs`, `lib.rs` user-type branch, `instance.rs` port):
     3 site ganti skip-buta → parse range/size; bentuk eksotik
     (`[string]`, `[$]`, `[*]`, key-type) tetap skip (helper baru
     `skip_extra_unpacked_dims` di `proc.rs`). JANGAN `advance()` `[` sebelum
     `parse_range()` (ia mengkonsumsi bracket sendiri — bug awal: semua dim
     lanjutan jatuh ke Err → blind-skip).
   - **Elaborasi** (`elaborator/mod.rs`): `array_dims=[d0..dn]`,
     `array_depth=Π dims`, `total_width=elem_width × Π`, init fill semua
     elemen; decl-init decompose via `flatten_array_init` (nested concat →
     flat row-major; daftar datar juga diterima); port array_dims di-set.
   - **Index fold** (`expr.rs` baca + `stmt.rs` tulis): `mat[i][j]` → SATU
     `ArrayIndex` dgn index gabungan `inner_idx×dims[c] + current_idx`,
     `elem_width = inner_width/dims[c]` (engine tak berubah — ArrayIndex
     flat: start = idx × ew). Index pertama pada multi-dim = ROW
     (`elem_width × Π dims[1..]`) — `mat[i]`/`m3d[0]` sub-array terbaca utuh.
   - Verifikasi: probe `mat2d.sv` → `m00=1 m01=2 m10=3 m11=4 mdyn=4`;
     `mat_edge.sv` (write dinamis 42, row 0x2A01, 3D `m3d[1][0][1]`=6,
     port `out_port[1][0]`=99); `.mv` multi-dim → sim OK; 4 test baru
     (`test_multidim_unpacked_array_{read_decl_init,write_dynamic,3d_and_row,port}`)
     — full workspace **2597 pass, 0 gagal**.

5. **`++`/`--` di ekspresi: nilai salah + side-effect hilang** — parser
   men-desugar `i++`/`++i` di level ekspresi jadi `BinaryOp Add/Sub` murni.
   Akibat: `k = i--` → k=4 (harusnya 5, nilai **lama**), `i` tak pernah
   berubah; `j = ++i` → j benar tapi `i` tak naik; `while (++i < 4)` tak
   pernah berhenti (n=100000). Fix F48: varian `Expr::IncDec` +
   `IrExpr::IncDec { read, lv, inc, pre }`, evaluator baca → tulis balik ±1
   → hasil baru (pre) / lama (post). LRM 1800 §11.4.1.

6. **Enum member dari typedef `$unit` tak resolve** —
   `typedef enum {IDLE,RUN} State;` di luar module (persis output `mgen`
   untuk typedef level file) → `s = RUN` gagal E2001 "signal not found".
   Hanya enum PACKAGE yang di-register ke `pkg_param_ctx`. Fix: pre-pass
   enum member `unit_typedefs` di `build_pkg_param_ctx` (counter di-reset
   per typedef, sama seperti enum package).

7. **Initializer port ANSI menimpa assignment `initial`** —
   `output logic [1:0] st = 0; initial st = 1;` berakhir `st=0`. Proses
   `port_init_*` di-push setelah `initial` user, jadi menimpa di delta yang
   sama. SV: inisialisasi variabel fase inisialisasi SEBELUM aktivitas
   prosedural t=0 (LRM 1800 §4.3.2/§6.8). Fix: proses port-init di-prepend
   ke daftar proses.

8. **`.mv`: nilai awal `reg` hilang bila nama sama dengan port** —
   `out a : Addr` + `reg a : Addr = 16'h2A` → reg di-skip sebagai deklarasi
   ganda (benar) tetapi initializer-nya ikut hilang → port `a` = X. Fix
   codegen: nilai awal dipindah ke deklarasi port (`output Addr a = 16'h2A`,
   sah LRM 1800 §6.8.2); input port tak boleh diinisialisasi → diabaikan.

9. **Struct assignment pattern `'{...}` menghasilkan 0** — `p = '{hi: 4'hA,
   lo: 4'h5}` pada struct packed berakhir `p = 00` (siluman), padahal member
   access `p.hi`/`p.lo` berfungsi penuh. Dua sebab: (a) arm `Expr::StructLit`
   di `elaborate_expr` tak tahu layout typedef → `FillLit(0)`; (b) even
   setelahnya, `apply_lhs_context_width` const-fold `Expr::StructLit` → 0
   menimpa lagi. Fix: elaborator baru `struct_lit.rs`
   (`elaborate_struct_pattern` + packing MSB-first dari `SignalInfo
   .struct_fields`), dipanggil di arm Blocking/NonBlockingAssign; const-fold
   dilewati untuk pola struct. Mendukung pola bernama, posisional,
   `default:`, sebagian, dan nested struct (LRM 1800 §7.9).

10. **Specifier `$display` tak lengkap** — `%.2f`/`%.0f`/`%0.3f` dicetak
    LITERAL (`%.2f`), `%g`/`%x`/`%X` tak dikenal, flag `%-5d` (rata kiri),
    width `%10s`/`%c`/`%%` tak didukung. Fix F51 di `simulator/util.rs`:
    parsing presisi + flag, arm `'h'|'H'|'x'|'X'`, `'g'|'G'`, `'c'`, `'%'`,
    helper `push_padded`, `normalize_exp` (eksponen `e+00` gaya C), dan
    inti formatter jadi fungsi bebas `format_display_core` (bisa diuji tanpa
    engine). Oracle iverilog: keenam specifier real kini identik.

11. **Plain `%d` tak right-justify; X/Z partial salah kapitalisasi** —
    `$display("%d", 1)` untuk `integer` tercetak `1` (minimal), sedangkan LRM
    1800 Tabel 21-3 (dan iverilog/VCS) mensyaratkan field selebar nilai
    maksimum tipe → `          1`. Selain itu grup hex/octal ber-X parsial
    tercetak `x` (harus `X`), dan `%d` ber-Z parsial tercetak `X` (harus
    `Z`). Fix F52: `default_dec_field_width` + kapitalisasi unknown parsial
    di `%d`/`%h`/`%x`/`%o`. Test lama yang bergantung perilaku non-LRM
    (`test_sformatf_basic`, `test_sformatf_multiple_args`, `test_fstrobe`,
    `test_fmonitor`) di-update ke `%0d` + ekspektasi LRM baru.

12. **Rujukan hierarkis ke modul teratas ditolak** — `$display("%0d",
    tb_d6.a)` / `force tb_d6.a = 9` gagal E2001 "hierarchical signal '…'
    not found for write". Path `top.<sig>` sah SV (LRM 1800 §12.4) tapi
    `hier_signal_map` hanya berisi alias `inst.port` dari flatten instance.
    Fix F53: alias `"<top>.<sig>"` untuk tiap sinyal top tanpa titik.

13. **Const-fold `case` mengabaikan digit X/Z** — `case (2'b1x)` dengan
    label `2'b1?` HIT (salah; iverilog: miss). Jalur const-fold
    `Stmt::Case` membandingkan label sebagai INTEGER lewat `parse_literal`,
    sehingga `x`/`z`/`?` dipetakan ke 0 dan `1x` dianggap sama dengan
    `1z`. Fix F53: `literal_has_unknown_bits` dipakai sebagai guard —
    case expr/label ber-unknown dipaksa jalur runtime 4-state.

14. **Konversi real↔integer rusak total** — `$rtoi`/`$itor` tak dikenal
    (hasil 0 + warning RT9003); `$display("%d", real_var)` mencetak
    bit-pattern f64 mentah (4620580627691444634 untuk 7.9, bukan 8);
    `int'(3.99)` = 515396076 (bukan 4); assignment `integer k = real_var`
    sama salahnya; `%f` dari integer salah baca bit-pattern. Fix F54:
    runtime `$rtoi`/`$itor` (round ties-away-from-zero), flag real pada
    argumen formatter, varian IR `RealConst` (penanda tipe real), serta
    cast & konversi implisit lewat sysfunc. `$rtoi` sengaja mengikuti LRM
    (round), bukan iverilog yang truncation.

15. **Literal real tanpa titik desimal tak di-lex** — `1e3`, `10e2`,
    `2E3` hilang/ngawur. Penyebab: lexer `mivon-parser` sudah mendukung
    `<digits>.<digits>[exp]`, TAPI pipeline `run`/`sim` memakai lexer KEDUA
    di `mivon-compiler/src/frontend/lexer.rs` yang hanya mengenali bentuk
    bertitik. Fix F54: bentuk eksponen tanpa titik ditambahkan di kedua
    lexer (dengan rollback agar `1e` tetap identifier).

16. **Associative array tak berfungsi** — `int m[string]; m["k"] = v`
    menulis hilang, `m["k"]` baca 0, dan `m.num()/exists()/delete()` gagal
    "cannot call method on unknown class" (iverilog tak mendukung assoc →
    semantik diambil dari LRM 1800 §7.9). Fix F54: `is_associative`
    diteruskan di elaborator (tadinya hardcoded `false` → signal
    diperlakukan array 1-elemen sehingga indeks ke luar rentang),
    `ArrayIndex` untuk assoc di jalur baca (`elaborator/expr.rs`) dan tulis
    (`elaborator/stmt.rs`), method assoc masuk dispatch array
    (`is_dynamic || is_queue || is_associative`), dan WR0014 dikecualikan
    untuk dynamic/associative (storage-nya HashMap, `init_val` tak pernah
    berubah).

17. **Hasil function inlining kehilangan signedness** — temp hasil inlining
    selalu bertipe `Logic` (UNSIGNED), sehingga
    `$display("%0d", max2(-3, -9))` tercetak 4294967293 (assignment ke
    variabel integer tetap benar karena resize terjadi di sana). Fix F55:
    `TempSignal` bertambah elemen `dtype`; return slot memakai
    `func.return_type`, temp arg/lokal memakai dtype aslinya.

18. **Variabel `static` reset tiap pemanggilan** —
    `function static int counter(); static int c = 0; c++; return c;`
    selalu mengembalikan 1 (inliner membuat temp per-call + mengulang
    initializer). LRM 1800 §8.21: variabel static punya lifetime modul.
    Fix F55: nama deterministik tanpa counter call (semua ekspansi berbagi
    signal), dedup deklarasi, dan initializer dipindah ke deklarasi.

19. **Parameter override tak memengaruhi generate loop** — `expand_all_generates`
    memutasi `design.modules` in-place memakai nilai param DEFAULT; instance
    yang meng-override me-clone AST yang generate-nya sudah ter-expand, jadi
    `gen_sub #(.W(8))` dengan `for (gi=0; gi<W; gi++)` hanya menghasilkan 4
    iterasi (bit atas output tetap X). Fix F56: snapshot `pristine_param_modules`
    (AST sebelum expand, hanya untuk modul ber-param + generate) dipakai
    flatten saat re-elaborasi per signature param, lalu generate di-expand
    ulang dengan nilai param instance tersebut.

20. **Default `parameter type T = logic [7:0]` selalu lebar 1** —
    `parse_type_expr` membuang packed range (`DataType` tak punya variant
    range), dan cabang `is_type_param` di `parse_param_list` mencari `[`
    SETELAH parse (tak pernah ada — sudah dikonsumsi) → `ParamDecl.range`
    selalu None → elaborator fallback `type_default.width()` = 1. Akibat:
    `T d;`/`T q;` 1-bit → `WR0102 port width mismatch` + output 0
    (`TB_TP_BROKEN q8=0 q16=0`, padahal F32 mengklaim OK — contoh
    `examples/mv/type_param.mv` gagal). Ditemukan saat sweep contoh `.mv`.
    Fix: `parse_type_expr_with_range` (range pertama dikembalikan, bukan
    dibuang) dipakai cabang type-param → `ParamDecl.range` terisi →
    `type_param_widths` = 8. Oracle: `TB_TP_OK q8=4 q16=8`.

21. **`priority/unique casez/casex` hanya exact-match** — parser membuang
    kind saat ada qualifier (`priority casez` → `Stmt::PriorityCase` tanpa
    kind → `CaseType::Priority` → engine `case_val_eq` exact). Akibat:
    `3'b101` vs label `3'b1??` tak cocok (iverilog: cocok) —
    `examples/mv/case_qualifiers.mv` gagal 2 error + verilator CASEWITHX
    (contoh juga salah pakai plain `case` untuk wildcard — diperbaiki jadi
    `casez`). Ditemukan saat sweep contoh `.mv`, diferensial vs iverilog.
    Fix: `CaseKind {Plain,X,Z}` di `Stmt::UniqueCase/PriorityCase/Unique0Case`
    + `CaseType::{Unique,Unique0,Priority}{X,Z}` + match `casex_eq`/`casez_eq`
    di engine (`block_control.rs`, `parallel.rs`). Contoh kini
    `CASEQ_OK`, verilator bersih.

22. **Override type param typedef tak pernah berlaku** — `is_type_token`
    false utk Ident sehingga `#(.T(Wide16))` ter-parse sbg VALUE override
    (`param_map[T]=0`, T tetap default 8-bit; e2e F32 `q16=8` tak sensitif
    lebar sehingga lolos!). Ditemukan saat menutup limitasi F33 (`T'(a)`
    lebar 1 → `q=0` bukan `0x34`): selidik override typedef lebih dulu.
    Fix: (a) `cur_type_param_widths` + `resolve_cast_name_width` baca lebih
    dulu → `T'(a)` lebar efektif; (b) re-bucket Ident/ScopedIdent utk pname
    type-param target di flatten; (c) `#(.T(logic[15:0]))`: struct baru
    `TypeParamAssign{dtype,range}` + `parse_type_expr_with_range` di
    instance (range sebelumnya dibuang `parse_type_expr`). Oracle:
    `TPOV/CT16/TPL2 q=0x800`, `CT q=0x34`; 3 test width-sensitive baru.

23. **Statement-level `assert/assume/cover property` gagal parse** — arm
    statement memakan keyword (`advance`) tapi lupa maju sesudah `property`
    (warisan F6, dikopi F66/F69): `is_ident` hanya peek → RAW parser
    `expect(LParen)` melihat `Ident(property)` → error membingungkan;
    varian AST `Stmt::AssertProperty/AssumeProperty/CoverProperty` sebagai
    statement TAK TERJANGKAU (termasuk bentuk pre-existing `assert`!).
    Ditemukan saat sweep matriks statement × blok. Fix: 1 baris
    `self.advance()` per arm (`mivon-mv/src/parser/stmt.rs`). Emisi +
    sim + check sudah benar (dipakai jalur module-level) — tanpa ubahan.

24. **Streaming concat `>>`/`<<` identik & default salah** — ketiga evaluator
    (IR serial, AST, DAG-parallel) kumpulkan bit LSB-first lalu balik urutan
    chunk utk KEDUA operator + default slice=1 bit: `{>>{a}}` full-reversal
    (harusnya identitas), `{>>8{a}}` byte-swap (harusnya identitas).
    Ditemukan saat hunt diferensial buglog; oracle runtime Verilator
    (bukan const-fold): `>>` identitas semua slice; `<<` balik urutan slice
    MSB-first (`<<`=bitrev, `<<2`=73ea, `<<4`=dcba, `<<8`=cdab utk abcd).
    Fix LRM 1800 §11.4.14 di 3 situs: stream MSB-first, `>>` pack urutan
    sama, `<<` balik urutan slice; test lama `>>8=CDAB`/`>>1=B3D5`
    dikoreksi + test noslice baru.

25. **`$past(v,n)` riwayat tercampur antar tick** — kunci histori hanya arg
    (`$past(cnt)` utk n=1,2,3 berbagi 1 deque) + cap n+1 per panggilan saling
    menggusur: tiap edge mendorong 3 salinan → p1=p2=p3=4 (harus 30/20/10
    utk cnt steps 10/20/30/40). Bonus: riwayat-kurang mengembalikan 0,
    bukan X (4-state). Ditemukan saat hunt diferensial buglog (probe tick
    1/2/3). Fix: kunci per (arg,n) + X-fill `LogicVec::new` saat
    `len<=n` (`engine/eval/expr.rs`). Oracle: model per-evaluasi cocok
    Verilator tick-1 (cyc3 p1=20); tick-2/3 Verilator tak support arg.

26. **Fungsi matematika real tak dikenal (`$sqrt`, `$ln`, ...)** — LRM 1800
    §20.8 `$sqrt(2.0)`/`$ln`/`$floor`/`$exp`/`$pow`/trigonometri jatuh ke
    "unsupported system function" → 0 + warning RT9003; plus
    `expr_approx_width` = 1 → warning WR0102 palsu `lhs=64, rhs=1` saat
    assign ke `real`. Ditemukan saat hunt diferensial buglog (probe vs
    iverilog: `sqrt=0.000000`, harusnya `1.414214`). Fix: arm SysFunc
    matematika real di `engine/eval/expr.rs` (hasil bit-pattern f64 64-bit
    spt `$itor`; argumen real→f64 / integer→f64) + helper
    `eval_sysfunc_real_arg` + `ir_expr_is_real` kenali hasilnya (nested) +
    lebar 64 (`$rtoi` → 32 sekalian) di `elaborator/stmt.rs`.

27. **`new[N](old)` dynamic array kehilangan data** — parser parse lalu
    BUANG expr copy (`_init`) → engine alokasi X fresh: grow `new[6](d)`
    maupun shrink `new[2](d)` berisi 0 semua (iverilog: elemen
    dipertahankan). Ditemukan saat hunt diferensial buglog (probe
    string/array). Fix: teruskan init sbg argumen ke-2 + engine salin
    min(old,new) bit rendah — elemen 0 di bit rendah (`parser/expr.rs`,
    `engine/eval/expr.rs`).

28. **`q[$]` baca elemen PERTAMA + `s[i]` baca bit (bukan byte)** —
    `$` jadi SysFunc tak dikenal → index 0 (`qlast`=10, harus 30);
    index string di-BitSelect 1-bit → bit LSB char (`s[0]`="h" → 0,
    harus 104; LRM 1800 §6.16.2: `s[i]` = byte). Ditemukan saat hunt
    diferensial buglog (probe queue/string vs iverilog/Verilator).
    Fix: `$` → `size-1` runtime via MethodCall di elaborator baca+tulis
    (dinamis/queue); string → RangeSelect/ExprPartSelect 8-bit baca+tulis
    (`elaborator/{expr,stmt}.rs`). Oracle: queue baca identik iverilog
    (tulis crash di iverilog); string identik Verilator (104,101,108,108,
    111); queue kosong tak crash.

29. **`release`/`deassign` kembalikan snapshot basi** — driver berubah
    saat forced (write ditahan) lalu release → restore nilai pra-force
    (d=1, release → 0; iverilog: 1 karena driver dihitung ulang).
    Ditemukan saat hunt diferensial buglog (probe force/release vs
    iverilog). Fix: `redrive_after_release` — jalankan ulang proses
    kombinasional penulis sinyal (atribusi netral anti-race) di 4 situs
    Release/Deassign (`engine/scheduler/block.rs`); `reg` tak tersentuh
    (tetap pegang forced, LRM §10.6.2).

## ⏳ Open

(tidak ada item open — semua bug historis sudah tertutup)