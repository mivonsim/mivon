# MICD — Mivon Incremental Compilation Database

> **Status dokumen: ditulis ulang Okt 2026** setelah rombak 6 fase (Fase 0–5).
> Dokumen ini menggambarkan arsitektur **as-built**, bukan visi awal.
> Visi awal + 15 kritik asli diringkas di §6 sebagai arsip (semua kritik
> berstatus di §4).

## 0. Riwayat rombak

| Fase | Komit | Isi |
|------|-------|-----|
| 0 | `8e75866` | `get_slice` zero-copy, preproc ref tanpa clone, rollback rebuild indeks verify + dirty flag lengkap |
| 1 | `0ef1f08` | `StageStore<K,V>` generik di atas satu engine (`cache/stage.rs`) |
| 2 | `2c84c94` | Mirror write-through verify/profile, timescale fix, legacy freeze |
| 2b | `37f54a1` | Hapus populate 12 kategori tak-terbaca (-844 baris) |
| 3 | `bc69a66` | Invalidasi per-stage; fingerprint binary turun jadi statistik |
| 4 | `7d682f7` | GC satu budget global + reklamasi pid; hapus test debug `tmp_` |
| 5 | `87dce92` | Evict selektif per-stage ganti wipe total |

Setiap fase: test pengaman baru + review AI independen + commit + push.
Hasil akhir: **350 test lib lolos, 0 gagal**, `cargo check --workspace` 0 warning.

## 1. Arsitektur saat ini

```text
Pipeline (preprocess…lexer…parser…elab…sim)
    │ tulis/baca artefak lewat setter bertipe (write-through)
    ▼
MicdDatabase (facade; crates/mivon-compiler/src/micd/mod.rs)
    ├─ state/<pid>/*.mdb  → metadata, graph, verify, diag, symbol, type, stats
    ├─ objects/<pid>/     → payload CAS (*.ast, *.preproc)
    ├─ cache/<pid>/       → 21 store seragam via StageStore<K,V>
    └─ registry.json      → statistik pid (fingerprint, revisi, waktu)
```

### 1.1 Layout direktori

```text
<db>/  (default .mivon/database/, override MIVON_MICD_DIR)
    VERSION                  — skema database (SCHEMA_VERSION = 5)
    registry.json            — pid → ProjectInfo + statistik (bukan gerbang)
    locks/<pid>.lock         — writer lock exclusive per project
    objects/<pid>/
        <hash>.ast           — Design bincode (AST_FORMAT_VERSION = 5)
        <hash>.preproc       — PreprocEntry (combined + timescale + segmen)
    state/<pid>/
        metadata.mdb         — manifest + FileMeta per file
        graph.mdb            — FileGraph (LZ4) + def/use level simbol
        verify.mdb           — VerifyResult multi-level hash
        diagnostics.mdb      — FileDiags per file
        symbol.mdb           — SymbolIndex
        types.mdb            — signature module
        stats.mdb            — StatsDb (profil build)
        journal.mdb          — intent transaksi (crash recovery)
        snapshots/build-NNN  — snapshot DAG (maks 16, merge-base didukung)
    cache/<pid>/<kategori>/
        manifest.mdb + index/index.mdb + objects/ + blobs/ + journal/ + stats/ + lock/
    precompiled/<pid>/       — artefak per module (VCS AN.DB analog)
```

Prinsip: payload immutable content-addressed (`objects/`, CAS blobs);
index mutable transaksional via journal (temp + rename + commit point tunggal).
`ast.mdb`/`preproc.mdb` gaya lama di `state/` disapu saat open (mati sejak CAS).

### 1.2 Komponen (satu file = satu tanggung jawab)

| File | Baris | Isi |
|------|-------|-----|
| `micd/mod.rs` | ~3680 | Facade `MicdDatabase`, open/save, setter mirror, evict selektif |
| `micd/format.rs` | 623 | MDB1: mmap + checksum + LZ4 per-store, `get_slice` zero-copy |
| `micd/metadata.rs` | 117 | FileMeta, manifest + `stage_schemas` |
| `micd/graph.rs` | 293 | FileGraph + reverse index + def/use simbol |
| `micd/verify.rs` | 235 | VerifyResult, multi-level hash, indeks O(1) ast/semantic |
| `micd/gc.rs` | ~850 | GC state + GC cache terpadu + reklamasi pid |
| `micd/snapshot.rs` | 410 | Snapshot DAG, prune, rollback (rebuild indeks) |
| `micd/txn.rs` | 183 | Journal + recovery |
| `micd/cache/stage.rs` | 587 | `StageStore<K,V>`, codec key/value, skema per-stage |
| `micd/cache/store.rs` | 741 | `CategoryStore`: CAS objects/blobs, LRU/TTL, sweep |
| `micd/cache/pipeline.rs` | ~1100 | Sisa populator (elaborate/generate/optimize) + payload dibaca tools |
| `micd/cache/mod.rs` | 507 | `CacheLayer`: 21 store + API bertipe |

### 1.3 Jalur tulis (write-through, satu jalur)

- `set_verify` → `verify.mdb` + cermin `cache/verify` (format `VerifyPayload`).
- `set_stats` → `stats.mdb` + cermin `cache/profile` key `"last"`.
- `cache_ast`/`cache_preprocessed` → objek CAS (skip bila hash sudah ada).
- `populate()` kini HANYA elaborate/generate/optimize (kategori dibaca tools).
- Tools (`msim`/`mcov`/`mlint`) tulis langsung kategorinya (Simulation/Waveform/Coverage/Lint).

### 1.4 Jalur baca panas (zero-copy, lazy)

- `get_ast` → `Arc<[u8]>` clone (refcount, bukan memcpy).
- `get_preprocessed_ref`/`combined` → `&` tanpa clone `timescale_segments`.
- Helper MDB (`read_mdb_singleton/entries`) pakai `get_slice` (mmap) + fallback `get` untuk store LZ4.
- Open lazy: payload objek dibaca on-demand; index di memori.

## 2. Gerbang invalidasi (berurutan di `open`)

1. `SCHEMA_VERSION` (state) / `CACHE_SCHEMA_VERSION` (cache) — beda → rebuild store terkait.
2. `COMPILER_VERSION` (`-p<N>`, base match; suffix fingerprint legacy diterima) — beda → wipe/rebuild.
3. `stage_schemas` per-stage — beda → gugur **hanya** direktori `cache/<stage>` terkait (+ `verify.mdb` bila stage `verify`); state lain selamat. Tak-dikenal (kosong) → wipe total (strict).
4. Content/AST/semantic hash per artefak (`reuse_verify`: content → ast → semantic, semua O(1)).
5. Sidik input elaborasi (Fase 6): IR cache diikat hash (path+konten+include+flags+top) + mode elaborasi via sidecar `irinputs:` — libfile/flag/header/mode berubah → elaborasi ulang, bukan IR basi.
5. `binary_fingerprint` (size+mtime exe) — **hanya statistik** di `registry.json` + peringatan debug. Tidak lagi me-wipe (Kritik C).

Aturan operasi: **bump `-p<N>` setiap output kompilasi untuk source sama berubah** (satu-satunya pelindung semantik); naikkan `stage_schema_version` saat format payload satu stage berubah; naikkan `SCHEMA_VERSION` saat layout state berubah.

## 3. GC terpadu + reklamasi (Kritik F)

- SATU budget global `512MB` per pid untuk 21 kategori (bobot proporsional 256/64/8 — dulu ±1,4GB independen). Penegakan global ketat: evict terlama lintas-kategori.
- State: 256MB AST + 64MB preproc + 50k verify + TTL 7 hari + compaction unreachable.
- Reklamasi pid tiap save (best-effort, konservatif): lewati pid sendiri + pid terkunci; hapus pid idle >30 hari atau melebihi 16 pid (state/objects/cache/precompiled/lock + registry).
- GC tak pernah hapus metadata/graph/symbol/type (inti incremental). Evict = rebuild otomatis (cost cold cache).

## 4. Status 15 kritik asli

| # | Kritik | Status |
|---|--------|--------|
| A | Duplikasi 4 pola penyimpanan | Selesai besar: populator 12 kat dihapus, mirror 2 kat, 1 engine seragam. Sisa: `pipeline.rs` elaborate/generate/optimize (dibaca tools) |
| B | God-object + clone panas + eager load + dirty flag | Selesai: zero-copy, ref API, `dirty_diag`, rollback lengkap. Sisa: `mod.rs` tetap ~3680 baris (facade, bukan dipecah) |
| C | Fingerprint mtime menghukum dev | Selesai (Fase 3): fingerprint = statistik; pid stabil; gerbang = base+schema+stage |
| D | Save transaksional tapi tak atomik antar-store | Partial: satu journal per project + commit point + recovery validasi checksum. Sisa: tanpa snapshot konsisten antar-store (didokumentasikan) |
| E | Key u64 tanpa cek kolusi + scan O(n) | Selesai besar: indeks byte-string diverifikasi saat load, indeks semantic O(1). Sisa: kunci MDB internal tetap u64 (identitas = string tersimpan) |
| F | Budget ×21, tanpa reklamasi | Selesai (Fase 4): satu budget global + reklamasi pid |
| 1 | Multi-level hash | Selesai: content→ast→semantic, O(1) semua |
| 2 | Graph level simbol | Selesai: def/use di graph.mdb |
| 5 | Crash recovery | Selesai: journal + recovery + sweep tmp |
| 6 | GC/LRU/TTL/compaction | Selesai + terpadu (Fase 4) |
| 7 | Lock | Selesai: 1 writer lock per project; reclaim hormati lock |
| 9 | Verify per kategori | Selesai: `checks` per `VerifyCheckKind` |
| 13 | Snapshot DAG | Selesai: parents/merge-base/prune + rollback rebuild indeks |
| 14 | Stats DB | Selesai + mirror profile |
| 3,4,8,10–12,15 | Schema version, txn, MVCC, API publik, dsb. | Selesai besar: SCHEMA 5 + CACHE 1 + AST 5 + IR 4; API via `mivon-compiler`. Sisa jujur: tanpa MVCC/distributed cache (non-goal internal) |

## 5. Kategori cache/: siapa mengisi, siapa membaca

Dibaca tools (diisi, dipertahankan): elaborate, generate (`melab`), optimize/expression (`minspect`), lint (`mlint` tulis/baca), coverage (`mcov`/`msim`), simulation/waveform (`msim`), profile (`mprof`, via mirror).

Mati/dihapus dari populator (tak ada pembaca produksi): preprocess, lexer, parser, semantic, macro, include, dependency, resolve, constant, type, hierarchy. Verify/profile via mirror setter.

## 6. Sisa pekerjaan (jujur, non-goal dicoret)

1. `pipeline.rs` sisa (~1100 baris) → write-through penuh + pecah payload per-stage + migrasi 6 tools (butuh rewrite import tools).
2. Registry read-modify-write tanpa lock global (pola pre-existing; reclaim + register balapan antar-proses).
3. Snapshot konsisten antar-store (Kritik D penuh) — butuh commit point multi-file.
4. Entri yatim 12 kategori mati di DB lama (tak di-prune; tak terbaca, hanya bobot disk).
5. Non-goal (internal): MVCC, distributed cache, Zstd (LZ4 cukup).
