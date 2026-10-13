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

30. **Nilai enum eksplisit tak-ter-evaluasi ditebak diam-diam** —
    `enum {A = TYPO_X, B}` mendaftarkan A=0/B=1 tanpa diagnostic
    (`unwrap_or(last)` di 4 situs: $unit/package/module-enum +
    precompute skalar package di `const_eval_ext.rs`); rujukan `s = A`
    mensimulasikan 0 padahal A tak terdefinisi. Lebih parah: guard
    `or_insert`/`contains_key` membekukan tebakan iterasi-1 sehingga
    forward-ref valid pun terkunci salah. Ditemukan saat review F80
    (divergensi vs `gen_const_value` .mv yang strict).
    Fix: skip-on-failure + rantai `Option` di semua situs, enum $unit
    masuk fixpoint (bukan sekali jalan), fallback overlay param
    package-sendiri untuk forward-ref valid, warning `ParamMismatch`
    pasca-fixpoint per typedef + E2001 jujur di use-site.

31. **Override parameter instance non-konstan ditebak 0** —
    `#(.W(dyn_w))` (sinyal sebagai override) ilegal LRM namun lolos
    diam-diam dengan W=0 (`unwrap_or(0)`). Fix: error `ParamMismatch`
    berposisi di titik override. Situs `param_util.rs` (fallback 1/0
    untuk default hilang/tak-ter-evaluasi) SENGAJA dibiarkan: heuristik
    keep-going terdokumentasi untuk desain nyata (OpenTitan).

32. **Step generate-for non-konstan ditebak 1** — `for (i=0; i<N; i+=s)`
    dengan `s` sinyal mengembang dengan step 1 diam-diam (hardware salah).
    Fix: `extract_generate_step` return `Result` + error `step eval failed`
    berposisi (jalur `expand_all_generates` menurunkannya jadi warning +
    blok di-skip, sesuai kebijakan anti-cascade).

33. **Count replikasi non-konstan ditebak 1** — `{n{...}}` dengan `n` sinyal
    melebarkan salah diam-diam (count selalu statis di IR, konsisten dengan
    `RangeSelect` yang sudah error). Fix: error eksplisit. Part-select
    dinamis SENGAJA tak disentuh (runtime `ExprPartSelect` mendukung lebar
    dinamis — superset deliberate). Follow-up: error di jalur implicit-net
    mematikan desain fuzz yang sebelumnya jalan → di sana warning +
    fallback 1 (jalur sudah degraded); width LHS dinamis (IR statis) juga
    warning `NotImplemented` + fallback.

34. **Lebar literal unsized berbasis = 1 (harus ≥32)** — `$bits('b101)`
    menghasilkan 1, padahal LRM §5.7.1 + iverilog = 32 (sama seperti
    desimal unsized yang sudah 32). Ditemukan saat audit sisa katalog
    `unwrap_or`. Fix: `compute_expr_width` + `sized_width`. Follow-up
    review: arm `$bits` const-eval mengembalikan NILAI arg (bukan lebar —
    `$bits('b101)` ter-fold 5!); kini memakai `sized_width`, `Err` bila
    tak terhitung. Batasan sadar: `$size` disamakan `$bits` di jalur const
    (runtime sudah bedakan benar: `$size` = elemen, `$bits` = total bit);
    `$left`/`$right`/`$low`/`$high` full-evaluator masih kembalikan nilai
    arg — butuh info tipe array yang tak tersedia di const-eval.

35. **Queue lewat typedef dianggap skalar (F40b)** — `typedef logic [7:0]
    Q [$];` (dimensi `[$]` **setelah nama typedef**) dibuang parser
    (`skip_typedef_unpacked_dims`) tanpa jejak, sehingga `Q q;` / `out q : Q`
    jadi **skalar** (lebar elemen): `q.size()` = 0, `q[0]` membaca bit sampah,
    dan `q = Q'(x)` (→ `'{ x }`) menyimpan lebar salah. Ditemukan dari buglog
    open #1 (`cast_queue.mv` → `TB_CASTQ_BROKEN qs=0 q0=1`). Fix lengkap:
    - **AST** (`mivon-ast/src/types.rs`): `TypedefDecl.is_queue` + `is_dynamic`
      (`#[serde(default)]`).
    - **Parser** (`mivon-parser/src/decl.rs`): `skip_typedef_unpacked_dims`
      kini mengembalikan `(is_queue, is_dynamic)` (`[$]`/`[$:N]` → queue,
      `[]` → dynamic); cabang user-defined menandai queue inline.
    - **Elaborator** (`elaborator/mod.rs`, `expr.rs`): set `typedef_queue`/
      `typedef_dynamic` di semua situs registrasi typedef; var bertipe
      `UserDefined` + port bertipe queue typedef dimodelkan sebagai queue
      (width 0, elem_width = lebar dasar, init kosong).
    - **Assignment pattern queue** (`elaborator/struct_lit.rs`): parser
      meng-desugar `'{a,b,c}` semua-posisional jadi `Expr::Concat` (MSB-first);
      untuk LHS queue/dynamic, operand di-repack terbalik agar **elemen 0 =
      operand pertama (bit terendah)**. Jalur `pack_array_pattern` juga
      menangani `Expr::Concat` (bukan hanya `Expr::StructLit`).
    - Verifikasi: `mivon sim examples/mv/queue_typedef.mv` → `QTD_OK`
      (push/cast/pattern/port); `cast_queue.mv` → `TB_CASTQ_OK r=a5 q0=a5`.
      3 test baru: `test_mv_queue_typedef_var_and_cast`,
      `test_queue_typedef_pattern_multi_elem`,
      `test_mv_queue_typedef_example_end_to_end`.
    - Sisa caveat (keterbatasan TOOL, bukan generasi): iverilog tak dukung
      queue sbg port module — verifikasi port via verilator/simulator mivon.

36. **Array method `with (item …)` tak bisa dipakai (F41)** — tiga bug di jalur
    array manipulation (LRM 1800 §7.12), ditemukan dari probe `q.find with
    (item > 3)` (verilator lint clean → bentuk ini SV valid):
    - **Parser SV** menolak bentuk LRM **tanpa kurung argumen** `q.find with
      (item > 3)` — E1005 "'with' clause can only follow a method call"
      (karena `q.find` ter-parse sebagai `MemberAccess`, bukan `MethodCall`).
    - **Elaborator** tak mengenal iterator implisit `item` di dalam clause →
      E2001 "signal 'item' not found" (bahkan untuk `q.find() with (…)`).
    - **Method bernama keyword** (`unique`/`and`/`or`/`xor`) gagal parse
      setelah `.` ("expected identifier, found unique") — lexer memetakannya
      ke token keyword.
    - **Semantik clause**: reduction/ordering memakai clause sbg BOOL
      (`check_with_clause`) — salah untuk clause bernilai (`q.sum with
      (item * 2)`).
    Fix:
    - **Parser SV** (`mivon-parser/src/expr.rs`): arm `Expr::MemberAccess
      {obj,field}` di blok `with` → `MethodCall{obj, method:field, args:[],
      with_clause}`.
    - **Parser SV + `.mv`** (`mivon-parser/src/lib.rs` `expect_member_name`;
      `mivon-mv/src/parser/mod.rs` `expect_member_name`): menerima
      `Token::Unique/And/Or/Xor` (`Tok::Unique/And/Or`) sbg nama member —
      HANYA setelah `.` (keyword tetap keyword di konteks lain).
    - **Elaborator** (`mivon-elaboration`): field `with_item_bound: Cell<bool>`
      + helper `elaborate_with_clause` (dipakai di 4 situs MethodCall);
      saat true, `Expr::Ident("item")` → `IrExpr::HierRef("item")`.
    - **Runtime** (`engine/eval/expr.rs`): `IrExpr::HierRef` fallback ke
      `get_local` (`method_locals`). `engine/uvm/util.rs`: `eval_with_value`
      (nilai clause per elemen) dipakai sum/product/and/or/xor/min/max/unique/
      sort/rsort; `check_with_clause` jadi wrapper bool.
    - **Semantik `unique`** (LRM §7.12.3): kini **mengembalikan queue baru** &
      tidak mengubah sumber — dulu write-in-place + return kosong (chained
      `.size()` → RT0001).
    - **`.mv`** (`mivon-mv`): lexer `Tok::With`; AST `MethodCall.with_clause`;
      parser arm `Tok::With`; codegen/print `q.find() with (…)`; check
      mengenali `item` (bukan E2001).
    - Verifikasi: `mivon sim examples/mv/array_methods.mv --top tb_arr` →
      `ARR_OK` (find/find_index/sum/min/max/sort/unique); verilator lint OK.
      3 test baru: `test_array_method_with_clause_item_iterator`,
      `test_array_method_keyword_names`,
      `test_mv_array_method_with_clause_end_to_end`.
    - Sisa caveat (keterbatasan, bukan bug): chained method pada hasil
      temporer (`q.unique().size()`) belum didukung — simpan ke queue dulu
      (`r = q.unique(); r.size()`); akses member/index pada `item`
      (`with (item.x)`) → **kini DIDUKUNG penuh, lihat #40**; iverilog tak
      dukung `with` clause.
    - **Follow-up auto-review F41:**
      - `min`/`max` (§7.12.1) dulu mengembalikan **nilai kunci** clause,
        bukan elemen — `q.min with (100-item)` salah. Kini mengembalikan
        elemen pemenang. Test: `test_array_method_minmax_key_and_find_last`.
      - `find_last`/`find_last_index` dulu mengembalikan SEMUA kecocokan
        (terbalik), bukan satu terakhir — kini `break` setelah kecocokan
        pertama saat men-scan mundur.
      - `unique_index` dibuang dari daftar `is_array_with_method` (runtime
        belum mengimplementasikannya — jangan overclaim). → **kini didukung
        penuh, lihat #39.**
      - Referensi §7.12.3 pada `unique` dikoreksi ke §7.12.1.
      - mivon-mv: walker sekunder (`hazard.rs` ×3, `check/expr.rs`
        `expr_uses`, `lib.rs` `collect_expr_refs`) kini menelusuri
        `with_clause` (sebelumnya di-skip lewat `..`).

37. **Frame `evaluate_expr_impl` ~184 KB → stack overflow rekursi (#37).** Rantai
    `&&` **≥8 operan** di evaluator rekursif (`use_packed_eval=false` — dipakai
    `EngineFlags::default()`, yaitu jalur test API `simulate_signals`) menabrak
    stack thread test 2 MiB. gdb: tiap level rekursi `evaluate_expr_impl`
    memakai **~0x2E240 ≈ 184 KB** (fungsi ~2874 baris; temporari antar match-arm
    tak berbagi slot stack di build debug). Kedalaman aman ≈ 7 kondisi; 8 kondisi
    → `SIGABRT`. Bukan bug semantik (nilai benar bila stack cukup): CLI
    `mivon sim` (main thread 8 MiB) & packed eval tidak terpengaruh.
    - **Fix (dua bagian; keduanya perlu):**
      1. **Dispatcher rekursif (DECISIVE).** `evaluate_expr_impl` lama (match
         raksasa ~2400 baris) di-rename jadi `evaluate_expr_deep`; lahir
         `evaluate_expr_impl` baru yang **tipis**: tulang belakang rekursif
         `BinaryOp`/`UnaryOp` ditangani di situ lewat helper
         `#[inline(never)] fn eval_binary_op` / `fn eval_unary_op`, sisanya
         didelegasikan ke `evaluate_expr_deep`. Karena `evaluate_expr_impl`
         (yang dipanggil tiap level rekursi) kini ber-frame kecil, rantai
         `&&`/`||` panjang **tidak lagi** mengalokasikan frame
         `evaluate_expr_deep` (~184 KB) per level rekursi.
      2. **Pecah arm `SysFunc`** (≈1320 baris, `match name.as_str() { ... }`)
         ke method `#[inline(never)] fn eval_sysfunc(&mut self, name, args,
         line, col)` — memangkas frame `evaluate_expr_deep` itu sendiri
         (bermanfaat utk pola rekursi LAIN yang masih menembus
         `evaluate_expr_deep`, mis. sysfunc bersarang / method bersarang).
         Body dipindah apa adanya (hanya `self.set_cur_src_pos(*line, *col)`
         → `(line, col)`).
    - **Bukti bagian (1) WAJIB:** dengan HANYA pemecahan `SysFunc` (bagian 2,
      tanpa dispatcher), rantai **8 operan MASIH `SIGABRT` stack overflow** di
      thread test 2 MiB — dispatcher (bagian 1) yang membuat test lulus. Pada
      helper, `lhs`/`rhs` kini `&IrExpr` (bukan `&Box<IrExpr>`) sehingga
      `lhs.as_ref()` → `lhs`.
    - Verifikasi: `examples/mv/logic_chain.mv` (rantai `&&`/`||` 16 operan) —
      test `test_mv_long_logic_chain_no_stack_overflow`; plus
      `test_deep_boolean_chain_no_stack_overflow` (8/12/16 operan via
      `simulate_signals`). Keduanya LULUS pasca-fix.
    - Catatan: fix ini TIDAK mengubah semantik apa pun — seluruh 281 test
      `mivon-mv` + suite `mivon-tests` (1092) hijau.

38. **Deep-recursion di ELABORATOR + const-eval — rantai ekspresi ≥~17 operan →
    stack overflow.** Ditemukan saat memperluas test #37: rantai `&&` 32 operan
    (semua operand SINYAL, bukan konstanta) masih `SIGABRT` di thread test 2 MiB,
    overflow BUKAN di evaluator melainkan di `mivon_ast::const_eval` +
    `Elaborator::elaborate_expr`. Tiga akar (ditemukan bertahap via gdb):
    - **(a) `elaborate_expr` memanggil `try_fold_const` di SETIAP level.** Untuk
      ekspresi ber-sinyal, `const_eval_with_params` menelusuri seluruh subtree
      (rekursif, frame besar) sebelum gagal → total frame O(N^2) saat menurunkan
      rantai. **Fix:** walker `util::const_fold::contains_signal_ref` (walk
      ringan) dipanggil di awal `Elaborator::try_fold_const`; ekspresi ber-sinyal
      langsung di-skip (semantik identik — fold ber-sinyal memang selalu None).
    - **(b) `const_eval_with_params` satu match ~944 baris** (frame besar/level).
      **Fix:** dispatcher tipis + `#[inline(never)] const_eval_ops` (BinaryOp/
      UnaryOp, jalur rekursi) + `const_eval_deep` (sisa arm, di luar jalur) —
      pola fix #37.
    - **(c) `elaborate_expr` (~920 baris) ber-rekursi di jalur rantai.**
      **Fix:** dispatcher tipis + `#[inline(never)] elaborate_binary`/
      `elaborate_unary` + `elaborate_expr_deep` (pola fix #37).
    - **Depth-guard** `const_eval_with_params` (`MAX_CONST_EVAL_DEPTH = 128`,
      thread-local counter): jalur lain (mis. `util/width.rs`) memanggil
      const-eval langsung tanpa guard sinyal → batas ini menjamin tak ada jalur
      yang overflow (ekspresi >128 dianggap bukan konstanta → `Err`).
    - **Batas terukur pasca-fix:** 16 → **128 operan LULUS** (evaluator +
      elaborator). Cliff berikutnya ada di SIMULATOR `eval_binary_op` (~256
      operan) — evaluator rekursif masih ber-frame sedang; bukan target bug ini
      (rantai ≥256 operan tak realistis).
    - Verifikasi: `test_mv_very_long_logic_chain_no_stack_overflow` (64 operan
      sinyal, dibuktikan GAGAL/SIGABRT pada 32 pra-fix, LULUS pasca-fix); #37
      test tetap hijau.

39. **`unique_index` belum didukung — keterbatasan → full support (LRM §7.12.1).**
    F41 (auto-review #36) sengaja membuang `unique_index` dari daftar
    `is_array_with_method` karena runtime belum mengimplementasikannya; hanya
    `unique` (queue NILAI) yang jalan. `q.unique_index()` jatuh ke arm fallback
    `evaluate_array_method` → `Err(NotImplemented)` ("unknown array/queue
    method"). LRM 1800 §7.12.1: `unique_index` mengembalikan queue **INDEKS**
    kemunculan pertama tiap nilai unik (tiap indeks 32-bit).
    - **Fix (2 titik, manual):**
      1. **Runtime** (`engine/uvm/util.rs`): arm `"unique_index"` baru persis
         setelah `"unique"` — iterasi elemen, dedup via `HashSet` atas kunci
         `eval_with_value(with_clause, &elem)?.to_u64()`, dan untuk tiap nilai
         unik dorong `LogicVec::from_u64(i, 32)` (indeks) ke queue hasil.
         Mengembalikan queue baru (tidak mengubah sumber), sama seperti
         `unique`.
      2. **Elaborator** (`elaborator/expr.rs`): tambah `"unique_index"` ke
         `is_array_with_method` (binding iterator `item` bila ditulis
         `q.unique_index with (item)`).
    - Parser tidak perlu diubah: `unique_index` **bukan** keyword SV (beda dgn
      `unique`), jadi sudah di-parse sebagai identifier method biasa.
    - `with` clause opsional didukung (LRM §7.12.1): `q.unique_index with
      (item / 2)` dedup atas **nilai clause** — membuktikan jalur
      `is_array_with_method("unique_index")` (binding `item`) benar-benar
      terpakai.
    - Verifikasi: `test_array_method_unique_index` (baru) — `q='{1,1,2,3,3}` →
      `q.unique_index()` = `{0,2,3}` (size 3); `test_array_method_unique_index_
      with_clause` (baru, review-fix) — `q.unique_index with (item/2)` =
      `{0,2}`. Demo e2e
      `examples/mv/array_methods.mv` diperluas: `mivon sim` →
      `ARR_UIDX n=3 i0=0 i1=2 i2=3` + `ARR_OK`. Test
      `test_mv_array_method_with_clause_end_to_end` diperluas (assert un2/ui0/
      ui1/ui2).

40. **Akses field struct pada iterator `item` belum didukung — keterbatasan →
    full support (LRM §7.12.1).** Caveat F41 (#36): klausa `with` hanya boleh
    memakai `item` utuh; `q.find with (item.x > 3)` di-emit jadi
    `IrExpr::HierRef("item.x")` (elaborator, cabang `MemberAccess` →
    `Ok(IrExpr::HierRef(base_name))` menggabung `item` + `x`) → runtime
    `HierRef` gagal → **RT0001 "hierarchical signal 'item.x' not found"**.
    Padahal elemen queue bisa berupa struct packed, dan field-nya wajar dipakai
    di locator/reduction/ordering.
    - **Fix (elaborator saja, 2 titik):**
      1. Field `Elaborator.with_item_fields: RefCell<Vec<StructFieldInfo>>` —
         field struct dari TIPE ELEMEN receiver. Diisi di
         `elaborate_with_clause` (helper `with_item_element_fields`: receiver
         `Ident` → `SignalInfo.struct_fields`, yang untuk queue-of-struct =
         field elemen) dan di-restore setelah clause (pola `with_item_bound`).
      2. Cabang `MemberAccess` (`Ok(IrExpr::HierRef(base_name))`): bila
         `base_name == "item"` dan field ada di `with_item_fields`, emit
         `IrExpr::ExprRangeSelect(HierRef("item"), msb, lsb)` (potong bit sesuai
         `f.offset`/`f.width`, + `Signed` bila field signed) alih-alih
         `HierRef("item.x")`. Runtime sudah mendukung `ExprRangeSelect` atas
         ekspresi apa pun (local `item` di `method_locals`).
    - Runtime **tidak berubah**; `StructFieldInfo` sudah menyimpan offset/width/
      is_signed dengan bit-order benar (member pertama packed struct = MSB).
    - Verifikasi: `test_array_method_with_item_field_access` (baru, SV) —
      `p_t q[$]` (`p_t = struct packed {logic[7:0] x; logic[7:0] y;}`),
      `q='{'{1,10},'{5,20},'{9,30}}` → `find with (item.x>3)` n=2,
      `sum with (item.y)`=60, `min with (item.x)`=266 (elemen {x=1,y=10}),
      `max with (item.y)`=2334, `find_index with (item.x==9)`=2. Demo e2e `.mv`
      `examples/mv/array_item_field.mv` → `ITF_OK`; test
      `test_mv_array_item_field_end_to_end`.

41. **Lebar field struct dari TIPE salah (member tanpa range eksplisit) — bug
    inti, ditemukan auto-review F86 (#40).** Member struct yang lebarnya dari
    TIPE, bukan range (`int y;`, `byte b;`, enum/typedef), dihitung **1** oleh
    akumulasi offset struct_fields DAN `DataType::width()`. Akibatnya member
    berikutnya (MSB) salah offset → `e.x` baca bit yang salah; lebar total
    struct pun salah → potongan bit di luar batas = X.
    - **Reproduksi:** `struct packed { logic[7:0] x; int y; } e; e='{7,42};`
      → `e.x` = **21** (harus 7), `q.sum with (item.x)` = **150** (harus 14),
      `find_index with (item.x==9)` = 0 (harus 1). Terkena juga typedef struct
      (`typedef ... p_t`) karena `typedef_field_map` memakai akumulasi sama.
    - **Fix (elaborator saja, 3 titik — semua memakai lebar hasil resolve):**
      1. `elaborator/mod.rs` (2 tempat: struct_fields signal + `typedef_field_map`):
         `offset += m.range.unwrap_or(1)` → `offset += f.width.max(1)` (lebar
         `StructFieldInfo` hasil `struct_field_from_member`, yang sudah resolve
         typedef/enum/int). Menyamai `compute_struct_fields_depth`.
      2. `elaborator/types.rs` `resolve_type_width`: arm baru `StructType |
         UnionType` → lebar = SUM (struct) / MAX (union) lebar field hasil
         `compute_struct_fields` (konsisten dgn offset member), bukan
         `DataType::width()` (range mentah). Fallback `dtype.width()` hanya bila
         field kosong.
    - **Tidak menyentuh `DataType::width()` (mivon-ast)** — context-free, tak
      bisa resolve typedef; sumber kebenaran = elaborator.
    - Verifikasi: `test_inline_struct_type_width_member_offsets` (baru, SV) —
      `sx`=7, `s`=14, `i0`=1 (PRA-FIX 21/150/0). Demo e2e `.mv`
      `examples/mv/struct_field_width.mv` → `SFW_OK`; test
      `test_mv_struct_field_width_end_to_end`. Full workspace test hijau.

## ⏳ Open

_(Tidak ada item open. Caveat chained method temporer & iverilog `with`
tercatat di #36 sbg keterbatasan tool/desain, bukan bug. `with (item.x)`
kini DIDUKUNG penuh — lihat #40.)_