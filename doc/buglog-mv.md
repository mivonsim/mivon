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

## ⏳ Open

(tidak ada item open — semua bug historis sudah tertutup)