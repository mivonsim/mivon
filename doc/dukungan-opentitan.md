# Dukungan E2E OpenTitan — Satu Masalah Utama

> Status: pertimbangan dukungan kampus / BRIN / Kedaireka
> Project: Mivon — simulator SystemVerilog Rust + Mivon HDL (.mv)

## Masalah utama

E2E OpenTitan full-chip mentok hardware laptop, bukan mentok kode.
Mivon valid di skala kecil-menengah. Bukti full-SoC belum bisa jalan di sini.

## Fakta laptop (terukur 13 Okt 2026)

| Komponen | Nilai |
|---|---|
| CPU | AMD Ryzen 5 3500U, 8 thread |
| RAM | 7.2 GB total, available 2.3 GB |
| Swap | 39 GB, terpakai 2.2 GB (RAM sudah tekor) |
| Disk | 233 GB, terisi 89%, sisa 25 GB |
| Build | `target/` saja 30 GB |

Catatan kode: satu file 21k baris makan 30–42 dtk (`src/main.rs:1130`).

## Beban E2E OpenTitan

| Item | Nilai |
|---|---|
| Filelist | `opentitan_rtl.f`, 3.920 file |
| Source SV | 3.943 file `.sv` di `opentitan/hw/`, 531 MB |
| Konfig | `configs/opentitan.toml` |
| Target | `top_earlgrey` SoC penuh |
| Mode | `StrictSimulation`, `expand_all_generates=true`, `opt_level=3`, `max_parse_steps=20jt` |

Artinya: parse paralel ribuan file + elaborasi full + sim + VCD dalam satu run.
Di 7 GB RAM = OOM / swap-thrash. Di sisa disk 25 GB = tidak muat MICD + VCD.

## Kebutuhan server minimal

| Tahap | Target | RAM | CPU | Disk bebas | Kegunaan |
|---|---|---|---|---|---|
| 1 (jalan di laptop) | 1 IP (`uart`, `gpio`) | 7 GB | 8 thread | 25 GB | smoke, demo kuliah |
| 2 (server kampus kecil) | `top_englishbreakfast` | 16 GB | 8–16 core | 100 GB | subsystem, TA mahasiswa |
| 3 (server kampus penuh) | `top_earlgrey` 3.920 file | 32–64 GB | 16+ core | 200 GB+ | E2E full + CI + paper validasi |

Dana bukan gaji. Hanya jam compute + storage + runner CI.

## Rencana bertahap

1. Smoke per-IP di laptop — sudah bisa.
2. Subsystem di server 16 GB — milestone bersama lab.
3. Full `top_earlgrey` di 32–64 GB — paper validasi pertama Mivon vs iverilog/verilator.

## Tawaran balik ke kampus

- Modul praktikum gratis (`.mv → .sv` via `mgen`).
- 10+ judul TA dari backlog Fxx.
- IKU: publikasi LRM-compliance + adopsi kuliah + MBKM riset.
- Minta awal kecil: 30 mnt demo + 1 kelas uji + surat dukungan Kedaireka/BRIN.
