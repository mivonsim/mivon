# node_cache — Cache Komputasi Filesystem-Native

> **Status:** diimplementasikan (crate `crates/mivon-node-cache`, 2026-11).
> **Menggantikan penuh MICD** (`crates/mivon-compiler/src/micd/` dihapus,
> `db.md` dihapus). Tidak ada database, tidak ada daemon, tidak ada index
> tersembunyi: filesystem adalah satu-satunya persistent storage.

---

## 1. Prinsip

| Prinsip | Penerapan |
|---|---|
| **Filesystem-native** | Semua state adalah file biasa: object, manifest, graph, reference, journal |
| **Content-addressed** | Artefak diberi nama `BLAKE3(isi)`; isi identik = satu file |
| **Identity-based validity** | Cache basi dicegah oleh identitas komputasi, bukan waktu/mtime |
| **Immutable** | Object, manifest, dan graph snapshot tidak pernah ditulis ulang |
| **Transactional** | Journal + atomic rename; crash tidak menghasilkan cache palsu |
| **Satu engine** | SV frontend, DSL, synthesis, dan VM memakai kontrak node yang sama |

Empat komponen inti:

1. **Node Resolver** — dari fingerprint ke artefak valid.
2. **Artifact Store** — object immutable content-addressed.
3. **Dependency Graph** — graph snapshot immutable + reference project.
4. **Transaction & Recovery** — journal, atomic commit, pemulihan crash.

GC dan scheduler adalah bagian internal, bukan subsystem mandiri.

---

## 2. Layout direktori

```text
<project>/node_cache/
├── objects/ab/cd/<content-hash>     artefak immutable (BLAKE3-256)
├── nodes/01/02/<node-id>            manifest node (biner versioned)
├── graphs/<graph-id>               graph snapshot immutable
├── refs/<project-id>               graph terakhir project
├── refs/<project-id>.snapshot.<id> snapshot rollback (menjaga graph hidup)
├── transactions/<txn-id>           journal commit
├── tmp/                            artefak sementara
├── locks/<node-id>.lock            lock per-node/per-object
├── precompiled/<project-id>/       artefak analisis per module (turunan)
├── cache/<project-id>/<cat>.bin    artefak pipeline per kategori (turunan)
└── FORMAT                          versi penyimpanan + algoritma hash
```

* Sharding dua level (`objects/ab/cd/…`, `nodes/01/02/…`) menghindari jutaan
  file dalam satu direktori.
* `FORMAT` berisi `storage_version` dan `hash=blake3-256`. Versi tidak dikenal
  → cache ditolak (bukan ditebak), pengguna menghapus cache.
* `precompiled/` dan `cache/` adalah artefak **turunan** untuk tool
  (`melab`, `minspect`, `mcov`, `mlint`, `msim`). Keduanya tidak pernah
  menentukan cache hit kompilasi.
* Tidak ada `metadata.mdb`, `state/`, `registry.json`, atau file MDB apa pun.

---

## 3. Dua hash, bukan satu

### A. Content hash

```text
CONTENT_ID = BLAKE3(artifact_bytes)
```

Mengidentifikasi isi artefak. Dua artefak identik berbagi identitas yang sama,
lintas project sekalipun. Object tidak mengetahui node mana yang memakainya.

### B. Node hash

```text
NODE_ID = BLAKE3(
    stage_identity + stage_version
  + key + input_content_ids
  + dependency_node_ids_and_outputs
  + configuration_id + toolchain_id + target_id + output_schema
)
```

Setiap field diberi panjang eksplisit agar batas antar-field tidak ambigu
(`"ab"+"c"` ≠ `"a"+"bc"`). Implementasi: `identity::NodeFingerprint`.

`NodeFingerprint::key_hash()` menyimpan hash 64-bit key di graph snapshot agar
lookup tidak perlu memuat key mentah.

---

## 4. Format manifest node

Manifest adalah record biner versioned (`format/manifest.rs`), bukan memory
layout Rust mentah dan bukan JSON:

```text
magic "MNOD0001"
node_id[32] · stage · stage_version · key
config · toolchain · target · schema
inputs[n][32] · dependencies[n](node_id, output_content_id)
outputs[n](kind, content_id, size, schema)
complete: u8
checksum BLAKE3(body)[32]
```

* Checksum di akhir memvalidasi integritas; node id di dalam manifest wajib
  sama dengan nama file.
* `complete=false` tidak pernah dipakai sebagai cache hit.
* Manifest **tidak boleh** dipublikasikan sebelum seluruh artefak output ada
  dan ukurannya cocok (`manifest::publish`).
* Format graph (`MGRF0001`), reference (`MREF0001`), dan journal (`MJRN0001`)
  mengikuti pola yang sama (magic + field little-endian + checksum).

---

## 5. Graph tanpa database

`graphs/<graph-id>` adalah snapshot immutable berisi:

* project, generation, flags, toolchain, catatan snapshot;
* node index project (`index`) — payload state (metadata file, verify,
  diagnostic, simbol, type index, dependency graph, statistik, daftar snapshot);
* daftar entry `(node_id, stage, key_hash)`.

`refs/<project-id>` menunjuk graph terakhir yang berhasil dipublikasikan.

Run berikutnya:

1. baca reference project;
2. muat graph snapshot;
3. resolve node berdasarkan identitas;
4. validasi dependency + artefak;
5. reuse node identik; hanya node yang berubah dihitung ulang.

Graph lama tidak pernah ditulis ulang. Snapshot rollback disimpan sebagai
reference terpisah (`<pid>.snapshot.<id>`) agar GC tidak menghapus graph yang
masih berguna.

---

## 6. Node pipeline yang dipakai compiler

| Stage | Key | Artefak |
|---|---|---|
| `preprocess` | path file | `PreprocEntry` (combined source + timescale + segmen) |
| `parse` | path file | AST bincode (`AST_FORMAT_VERSION`) |
| `elaborate` | `top + input_hash + context` | `IrDesign` bincode (`IR_FORMAT_VERSION`) |
| `index` | `project` | metadata file, verify, diagnostic, simbol, type/deps, statistik, snapshot |

Identitas setiap stage memasukkan hash konten input dan fingerprint toolchain,
sehingga:

* perubahan isi file → node `parse`/`preprocess` baru (node lama tetap ada
  sampai GC);
* perubahan flags (`-D`, `-I`) → seluruh node file invalid;
* upgrade compiler/skema → artefak lama ditolak, bukan salah-decode;
* mode elaborasi berbeda → IR tidak dipakai silang.

`ProjectCache` (`project.rs`) adalah facade pipeline: `attach_cache`,
`save_cache`, `get_ast`, `cache_preprocessed`, `deps_unchanged`,
`elaboration_inputs_hash`, `restore_elaborate_ir`, `snapshot`, `gc`.

---

## 7. Empat lapisan validasi (anti cache basi)

| Lapisan | Yang diperiksa |
|---|---|
| Input | Isi source + hash seluruh header include (bukan mtime/ukuran) |
| Configuration | Defines, incdir, target, mode elaborasi |
| Implementation | Stage, versi stage, toolchain, skema payload |
| Artifact | Keberadaan object, ukuran, checksum content-addressed |

`dependency::includes_valid` memverifikasi header include terhadap isi disk
sekarang: file include yang hilang/berubah menggagalkan reuse preprocessed
output (output lama meng-embed header lama). Mutasi dengan ukuran sama dan
timestamp yang dimanipulasi tetap terdeteksi karena yang dibandingkan adalah
hash isi.

---

## 8. Run 1, 2, dan 3

```text
Run 1 (COLD)  MISS → compute → object + manifest → graph → ref
Run 2 (WARM)  HIT  → manifest + artefak dimuat, tidak menghitung ulang
Run 3 (WARM)  HIT  → tidak ada invalidasi hanya karena cache dipakai
```

Jika hanya satu file berubah:

```text
file A   MISS
file B   HIT
file C   HIT
IR top   MISS (karena input hash berubah)
```

Tidak ada aturan "semua pipeline harus diulang karena satu file berubah".

---

## 9. Concurrency, atomic commit, recovery

* **Per-node lock** — `locks/<id>.lock` dibuat dengan `create_new`. Dua worker
  yang menghitung isi sama berbagi satu artefak; yang kalah lock memakai hasil
  yang sudah dipublikasikan.
* **Urutan publikasi** — object → manifest → graph → journal → reference
  (atomic rename + fsync) → hapus journal. Rename dilakukan di direktori
  tujuan agar tetap dalam filesystem yang sama.
* **Recovery** — saat project dibuka:
  * journal yang graph-nya sudah dirujuk → dibuang;
  * journal dengan graph lengkap tetapi reference belum pindah → commit
    diselesaikan (idempoten);
  * journal tidak lengkap → dibuang; reference lama tetap valid;
  * `tmp/` dan `.tmp-*` dibersihkan.
* Crash tidak pernah menghasilkan reference ke graph setengah jadi.

---

## 10. Garbage collection

Mark-and-sweep (`gc.rs`):

1. baca seluruh reference project (termasuk snapshot);
2. tandai graph, node, dan artefak turunannya;
3. hapus object/node/graph yang tidak dirujuk;
4. bersihkan `tmp/` dan lock basi (>1 jam).

GC menunda pembersihan bila ada transaksi yang belum selesai, sehingga tidak
menghapus artefak di tengah commit. Object content-addressed yang dipakai
lintas project ikut dipertahankan selama ada referensi.

---

## 11. Struktur crate

```text
crates/mivon-node-cache/src/
├── lib.rs             permukaan publik
├── identity.rs        ContentId + NodeFingerprint/NodeIdentity (BLAKE3)
├── filesystem.rs      layout, sharding, atomic write, fsync, lock, FORMAT
├── format/mod.rs      encoder/decoder biner stabil + checksum
├── manifest.rs        manifest node + store + publish
├── object.rs          object store content-addressed (put/get/sweep)
├── graph.rs           graph snapshot + reference project
├── dependency.rs      hash isi file, validasi include/dependency
├── resolver.rs        cache hit/miss + validasi artefak
├── transaction.rs     journal + commit atomic + abort
├── recovery.rs        pemulihan crash idempoten
├── gc.rs              mark-and-sweep
├── scheduler.rs       batch paralel internal (std threads)
├── entries.rs         payload entri (FileMeta, VerifyResult, simbol, statistik)
├── ast.rs             serialisasi AST/IR + versi skema
├── pipeline.rs        payload artefak pipeline (lint/coverage/simulation/…)
├── cache_layer.rs     artefak pipeline per kategori (dibaca tool)
├── precompiled.rs     artefak analisis per module (dibaca tool)
└── project.rs         ProjectCache facade + ProjectIndex
```

Satu crate, banyak modul — dipecah hanya bila ada kebutuhan nyata.

---

## 12. Integrasi

* `crates/mivon-compiler/src/lib.rs` — `pub use mivon_node_cache as node_cache;`
* `CompileSession` — field `node_cache: Option<ProjectCache>`;
  `attach_cache()` (restore AST/preprocess), `open_cache_no_restore()`
  (`--recompile`), `save_cache()`, `cache_mark_elaborated()`,
  `cache_elaboration_inputs_hash()`, `save_elaborate_cache()`.
* `run_fast` — restore AST penuh (lex/parse di-skip).
* `run` (legacy) — reuse combined source (preprocessor di-skip) + IR cache.
* `mivon-env` — `DatabaseContext` membungkus `ProjectCache`.
* Tool — `minspect cache`, `melab --from-cache`, `mcov`, `mlint`, `msim`
  memakai `CacheLayer`/`PrecompiledDb` untuk artefak turunan.

**Root:** `<dir kerja>/node_cache` (langsung di root project, bukan di dalam
`.mivon`). Override: `MIVON_NODE_CACHE_DIR`. `--target-dir <dir>` →
`<dir>/node_cache`.

**Flag:**

| Flag | Efek |
|---|---|
| `--recompile` | buka cache tanpa restore (full rebuild), hasil tetap disimpan |
| `--cache-clear` | hapus cache SEBELUM restore, lalu dibangun ulang |
| `mivon clean` | hapus seluruh root node_cache |
| `MIVON_DEBUG_NODE_CACHE=1` | diagnostik attach/IR/recovery |

---

## 13. Pengujian

Unit test crate (`cargo test -p mivon-node-cache`) mencakup:

| Pengujian | Kriteria |
|---|---|
| Cold → warm → warm | run 2 dan run 3 sama-sama HIT |
| Mutasi source | hanya node terdampak yang MISS |
| Mutasi ukuran sama | hash isi mendeteksi perubahan |
| Manipulasi timestamp | tidak menghasilkan hit palsu |
| Upgrade compiler/skema | artefak tidak kompatibel ditolak |
| Object rusak | ditolak (checksum content-addressed) |
| Recovery journal | commit diselesaikan / dibuang tanpa cache palsu |
| GC | artefak yang masih dirujuk tidak dihapus |
| Cross-project | object identik dipakai ulang tanpa tulis ganda |
| Snapshot | reference snapshot dipulihkan dan graph tetap hidup |
| Cache layer | payload persisten; blob rusak diabaikan |

Benchmark terpisah untuk cold, warm, warm+1 file berubah, dependency besar,
lookup jutaan node, storage overhead, peak RAM, dan I/O hashing. Target
persentase tidak ditetapkan sebelum baseline tersedia: **benar dulu, baru
cepat**.

---

## 14. Yang sengaja TIDAK dilakukan

* Tidak ada database (SQL, KV, embedded) dan tidak ada daemon.
* Tidak ada index mutable global yang ditulis ulang tiap build.
* Tidak ada kompresi otomatis seluruh artefak (ukur CPU vs I/O lebih dulu).
* Tidak ada memory mapping default untuk artefak kecil.
* Tidak ada hashing ulang artefak yang baru diverifikasi dalam proses yang sama.
* Tidak menyimpan seluruh AST/IR di RAM hanya demi cache.
* Tidak memakai mtime/ukuran sebagai bukti tunggal source tidak berubah.
* Hasil runtime yang bergantung pada event queue/seed/state simulasi tidak
  dianggap artefak kompilasi yang bisa dipakai ulang.

---

## 15. Migrasi dari MICD

| Aspek | MICD (dihapus) | node_cache |
|---|---|---|
| Storage | `state/<pid>/*.mdb` + `objects/<pid>/` | `objects/`, `nodes/`, `graphs/`, `refs/` |
| Format | MDB1 + LZ4 | biner versioned + checksum, tanpa kompresi |
| Index | mutable, ditulis ulang per build | graph snapshot immutable |
| Identitas | content hash xxh3 + path | content hash BLAKE3 + node fingerprint |
| Commit | tulis beberapa store + journal | journal + rename reference atomik |
| GC | TTL/LRU per store | mark-and-sweep berbasis reference |
| Env | `MIVON_MICD_DIR` | `MIVON_NODE_CACHE_DIR` |
| Root | `.mivon/database` | `node_cache/` (root project) |

Nama lama pada API sesi juga dihapus: `attach_micd` → `attach_cache`,
`save_micd` → `save_cache`, `micd_mark_elaborated` → `cache_mark_elaborated`.
