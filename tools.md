11 CLI Tools Mivon
1. minspect

Mivon Inspect

Tool untuk menginspeksi isi project tanpa melakukan compile penuh.

Fungsi:

Menampilkan daftar module.
Menampilkan package.
Menampilkan interface.
Menampilkan class.
Menampilkan typedef.
Menampilkan dependency antar modul.
Menampilkan top module yang ditemukan.
Menampilkan statistik project.

Contoh:

mivon minspect opentitan.f

Output:

Project Statistics
────────────────────────

Modules      : 3,972
Interfaces   : 84
Packages     : 119
Classes      : 162
Generate     : 1,438
Parameters   : 8,951

Top Modules
────────────
top_earlgrey
chip_earlgrey
...

Largest Module
──────────────
prim_ram_2p

Average LOC/module
──────────────────
241

Subcommand:

mivon minspect stats
mivon minspect modules
mivon minspect hierarchy
mivon minspect packages
mivon minspect classes
mivon minspect interfaces
mivon minspect parameters
mivon minspect deps
mivon minspect cache

Subcommand `cache` membaca lapisan `cache/<pid>/` (db.md) tanpa compile —
menampilkan statistik per kategori (entries, bytes, hits, misses, hit-rate,
rebuilt) beserta ringkasan lapisan. Berguna untuk memeriksa apakah pipeline
cache (lexer/parser/semantic/type/dst.) benar-benar terisi dan di-reuse.

Kategori yang diisi otomatis saat compile: preprocess/, lexer/ (token
stream), parser/, macro/, include/, dependency/, resolve/, semantic/, type/,
constant/, hierarchy/, verify/, profile/. Setelah elaborasi, elaborate/
(instance + port binding + parameter override + proses + net resolution dari
IR) dan generate/ (blok if/for/case + instance hasil ekspansi) ikut terisi
(db.md "5. elaborate/", "16. generate/").

Cache yang sudah terisi dapat DIBACA ulang tanpa compile ulang:

- `mivon melab file.sv --from-cache` — hierarki/instance/port binding/
  parameter override/proses/net dari cache elaborate/ + generate/, tanpa
  menjalankan elaborator (db.md "1000 instance generate tidak perlu
  dielaborasi ulang").
- `mivon mprof file.sv --cached` — profil + bottleneck + rekomendasi dari
  build terakhir (db.md "20. profile/" — Mivon mengetahui bottleneck sendiri),
  tanpa menjalankan pipeline.
- `mivon minspect file.sv cache` — selain statistik per kategori, menampilkan
  isi lint/ (temuan `mlint`), coverage/ (hasil `mcov`/`msim --coverage`),
  simulation/ (initial state + scheduler + sensitivity list), waveform/
  (signal index top: nama, lebar, kind, net), optimize/ (const fold + loop
  unroll) dan expression/ (evaluasi ekspresi + sampel hasil fold) langsung
  dari cache, tanpa menjalankan tool/simulasi ulang.

Tool juga MENULIS hasilnya ke cache: `mlint` → lint/"report", `mcov` dan
`msim --coverage` → coverage/"last" (db.md "7. verify/ → lint/",
"19. coverage/"). Setiap `msim` (dengan atau tanpa --coverage) juga menulis
simulation/"last" dan waveform/"last" — initial state, event scheduler,
sensitivity list, dan signal index agar VCD/FST lebih cepat dibuka (db.md
"17. simulation/", "18. waveform/"). Setelah elaborasi (jalur `run`
standar & `--fast`), optimize/"last" dan expression/"last" ikut terisi dari
statistik elaborator: jumlah constant folding, loop for yang di-unroll +
statement hasilnya, jumlah evaluasi ekspresi (`elaborate_expr`), dan sampel
hasil fold (`WIDTH*8 → 256`) — db.md "6. optimize/", "10. expression/".

Contoh output `mivon minspect cache test/counter.sv`:

── Pipeline Cache ──
  root                       .mivon/database
  project id                 029877faa3cede04
  files                      1

  Category      Entries        Bytes    Hits  Misses    Hit%
  lexer/              1        479 B       0       0      0%
  parser/             1         40 B       0       0      0%
  semantic/           1        117 B       0       0      0%
  ...

── Cache Summary ──
  categories                 21
  entries                    12
  hit rate                   0%

Kenapa lebih berguna?

Saat menghadapi proyek besar seperti OpenTitan, sering kali Anda hanya ingin mengetahui struktur proyek atau memastikan modul tertentu memang terdeteksi, tanpa membuang waktu menjalankan elaboration atau simulation. minspect memberikan "X-ray" terhadap proyek dengan cepat, memanfaatkan indeks internal Mivon yang sudah ada.
2. mlint

Static RTL Linter.

Fitur

RTL lint
FSM check
Combinational loop
Latch detection
Width mismatch
Unused signal
mivon mlint rtl/
3. melab

Standalone Elaborator.

Hanya melakukan

parameter resolve
generate
hierarchy
mivon melab top.sv
mivon melab top.sv --reset-domain   # SIM-22: analisis reset-domain crossing
                                    # (domain reset + crossing sinyal antar
                                    # domain, severity OK/MEDIUM/HIGH/CRITICAL)
4. msim

Simulator.

mivon msim

Support

VCD
Wave
Assertion
Coverage
5. mcov

Coverage Analyzer.

Menghasilkan

coverage.html
coverage.json

Jenis

Line
Toggle
FSM
Branch
Assertion
6. mwave

Wave Utility.

Bukan viewer.

Digunakan

mwave merge
mwave export
mwave filter
mwave compare a.vcd b.vcd   # bandingkan 2 VCD (WAV-09): mismatch per signal
                             # + sinyal yang hanya ada di salah satu file
mwave search t.vcd "c*"     # cari sinyal by wildcard (WAV-10): * dan ?
mwave tree t.vcd            # index hierarki scope + sinyal (WAV-08)
mwave stats t.vcd           # statistik per sinyal (WAV-17): toggle, transitions,
                             # first/last change, activity% + stuck detection
mwave get t.vcd cnt --at 47 # random access nilai sinyal (WAV-07): sample di waktu T
mwave get t.vcd "c*"        # timeline penuh (tanpa --at/--range), wildcard * ?
mwave get t.vcd cnt --range 20:40  # semua perubahan dalam [t1, t2]
7. mfmt

Formatter.

Mirip

cargo fmt

Tetapi untuk

Verilog
SystemVerilog

Belum mendukung Mivon HDL (`.mv`) — `mfmt` memakai lexer SV
(`mivon_parser::lexer`) sehingga file `.mv` akan diformat sebagai SV dan
menghasilkan hasil yang salah. Untuk `.mv` pakepakai `mivon mgen` (menulis
`.sv`/`.svh` ber-format) atau `examples/mv` sebagai acuan format. Mendukung
`.mv` ada di roadmap (`mfmt` backend `mivon_mv`).

8. mgen

Generator SystemVerilog dari Mivon HDL (`.mv`/`.mvh`).

Rantai penuh

.mv → lex → parse → type-check (E2001–E2013) → codegen → .sv + .svh

Semua konstruk `.mv` punya mapping SystemVerilog yang deterministik dan
LRM-1800-compliant (lihat MIVON-HDL.md §10). `mivon run counter.mv` juga
menerima `.mv` langsung — di-transpile on-the-fly ke buffer, tanpa menulis
file.

mivon mgen counter.mv                    # counter.sv + counter.svh
mivon mgen src/ -o build/gen             # scan rekursif (lewati negative/, tersembunyi, prefiks _)
mivon mgen a.mv b.mv --check             # exit 1 bila output basi (CI)
mivon mgen counter.mv --stdout           # print ke stdout
mivon mgen counter.mv --package chip_types  # bungkus typedef level file dalam package
mivon mgen counter.mv --no-check         # lewati type-check (konstruk eksternal/UVM)
mivon mgen counter.mv --svh-only | --sv-only
mivon mgen counter.mv --verbose

Catatan: konteks `type-check` gabungan dibatasi per direktori, jadi dua
subdirektori yang sama-sama mendefinisikan nama tipe yang sama tetap
independen. Nama function/task level file bersifat global dalam satu
konteks (E2007 bila bentrok) — pakai `package` atau module-level `func`
bila butuh nama yang sama di unit berbeda.

9. mprof

Performance Profiler.

Output

Lexer

Parser

Elaboration

Optimization

Simulation

Bottleneck langsung terlihat.

10. mcheck

Project Health Checker.

Memeriksa

Missing file
Circular include
Dependency
Version
Config

PARSER-13 (AST differential): `mcheck a.sv --ast-diff b.sv` — bandingkan
AST elaborasi dua file (module/signal/proses/class/covergroup), exit 0
identik / 1 berbeda (regression gate).
11. mbench

Benchmark Tool.

mbench opentitan

Output

Compile speed

Memory

CPU

Cache hit

Parser throughput
12. synth

Synthesis Tool (SYNTHESIS.md — flow ala Vivado). Nama lama: `msynth` (alias).

Fungsi

Synthesizability check (SYN-1..9)
Lowering RTL → SIR (node-based, `mivon-sir`)
Pass manager optimizer (const fold, arith, mux, CSE, DCE)
SIR → generic netlist (`mivon-netlist` — 1-driver/N-load DAG)
Tech mapping → LUT6/CARRY4/FF (`--tech-map`, phase 4)
STA + area (`--timing` + `--constraint .mcs`, phase 5)
Liberty `.lib` parser → `.libmdb` (mivon-tech, phase 6 — fondasi ASIC mapping)
Netlist `.mvnet` / `netlist.v` / `netlist.json` / `tech.v`
Timing report (`timing.rpt`) & area report (`area.rpt`)
Utilization report

Contoh

mivon synth rtl/counter.sv --top counter --emit-mvnet
mivon synth rtl/ --check-only
mivon synth rtl/counter.sv --dump-sir
mivon synth rtl/alu.sv --dump-sir-opt --preset generic
mivon synth rtl/counter.sv --top counter --dump-netlist
mivon synth rtl/counter.sv --top counter --emit-netlist
mivon synth rtl/alu.sv --top alu --tech-map
mivon synth rtl/counter.sv --top counter --tech-map --preset fpga
mivon synth rtl/alu.sv --top alu --tech-map --timing --constraint chip.mcs

Output

Skor sintesizability
Dump SIR sebelum & setelah optimasi (--dump-sir / --dump-sir-opt)
Dump netlist generik (--dump-netlist)
Emit netlist ke file: `counter.netlist.v` / `.mvnet` / `.json` (--emit-netlist)
Tech mapping → `alu.tech.v` / `.json` / `.mvnet` + LUT/CARRY4/FF (--tech-map)
STA → `alu.timing.rpt`: WNS/TNS/critical path (--timing --constraint .mcs)
Area → `alu.area.rpt`: LUT/CARRY4/FF + unit area
Pass manager: const fold, arith, mux, CSE, DCE (--preset generic|fpga|asic|custom)
FF / LUT / CARRY4 / BRAM / DSP
Netlist gate-level — bisa disimulasikan engine Mivon (hasil = sim RTL)
10 GUI Tools Mivon
1. Project Explorer

Panel kiri.

Isi

RTL

Testbench

Package

Interface

Library

IP
2. Hierarchy Explorer

Menampilkan

Top

Module

Instance

Generate Block

Tree interaktif.

3. Signal Browser

Semua

wire

logic

reg

interface

parameter

Bisa search realtime.

4. Wave Studio

Viewer bawaan.

Fitur

Zoom
Marker
Trigger
Compare
Bookmark
5. Diagnostics Center

Semua

Warning

Error

Note

Hint

Klik langsung menuju source code.

6. Performance Dashboard

Panel performa compile.

Grafik

Lexer

Parser

Elaboration

Optimization

Simulation

Realtime.

7. Coverage Studio

Visualisasi

Heatmap coverage
Branch
Toggle
FSM
Assertion

Modul dengan coverage rendah langsung disorot.

8. Dependency Graph

Visual graph.

Menampilkan

Module A

↓

Module B

↓

Package C

Memudahkan memahami proyek OpenTitan yang memiliki ribuan modul, tanpa harus membaca include dan instansiasi satu per satu seperti sedang memecahkan teka-teki buatan orang yang membenci dokumentasi.

9. Memory & Cache Monitor

Khusus Mivon.

Menampilkan

Cache hit

Cache miss

Arena allocator

AST

HIR

IR

Database

Memory

Realtime.

10. Compile Timeline

Timeline horizontal seperti profiler modern.

Discover

████

Preprocess

██████

Lexer

██████

Parser

██████████

Elaboration

████████████████

Optimization

████

Simulation

██

Klik salah satu blok akan membuka detail waktu, penggunaan CPU, memori, jumlah modul, dan statistik internal. Ini sangat membantu menemukan bottleneck pada proyek besar seperti OpenTitan tanpa harus mengandalkan log terminal yang panjangnya bisa menyaingi novel.

Arsitektur yang disarankan

Seluruh tool CLI dan GUI sebaiknya memakai backend yang sama agar tidak ada duplikasi logika:

                 Mivon Core
                      │
      ┌───────────────┼───────────────┐
      │               │               │
   Parser         Elaborator      Simulator
      │               │               │
      ├───────────────┼───────────────┤
      │         MICD Database         │
      └───────────────┼───────────────┘
                      │
         ┌────────────┴────────────┐
         │                         │
      CLI Tools                GUI Panels
 (mlint, melab, ...)      (Wave, Timeline,
                           Coverage, dll.)
