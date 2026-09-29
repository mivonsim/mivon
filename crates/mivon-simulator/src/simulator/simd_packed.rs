//! SIMD-accelerated 4-state bitwise operations untuk PackedLogicVec.
//!
//! # Strategi
//!
//! PackedLogicVec menyimpan data sebagai `Vec<(u64, u64)>` (known, value per chunk).
//! Untuk sinyal multi-chunk (>64 bit), operasi bitwise di-loop per chunk.
//! SIMD memproses 4 chunk (AVX2) atau 8 chunk (AVX-512) per instruksi CPU.
//!
//! # Dispatch
//!
//! - `len >= 8`: coba AVX-512 > AVX2 > scalar
//! - `len < 8`: langsung scalar (SIMD overhead > benefit untuk chunk sedikit)
//!
//! # Keamanan
//!
//! Semua fungsi SIMD menggunakan `unsafe` intrinsics.
//! CPU feature detection via `is_x86_feature_detected!` di runtime.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::simulator::types::XPropagationMode;
use crate::simulator::value::get_xprop_mode;

/// True bila mode berjalan X-dominan untuk bitwise AND/OR (SIM-11):
/// `0 & X = X`, `1 | X = X` (deduksi nilai tak boleh menutup X).
/// Optimistic/LRM: `0 & X = 0`, `1 | X = 1` (tabel 4-state standar).
#[inline]
fn x_dominant_bitwise() -> bool {
    let m = get_xprop_mode();
    m == XPropagationMode::Pessimistic || m == XPropagationMode::XAnywhere
}

// ─── Public API ───

/// SIMD-accelerated 4-state AND untuk array chunks.
///
/// Input: `Vec<(known, value)>` per operand.
/// Output: `Vec<(known, value)>` hasil AND.
pub fn simd_and(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len().max(b.len());
    if len < 8 {
        return scalar_and(a, b);
    }
    simd_dispatch_and(a, b, len)
}

/// SIMD-accelerated 4-state OR untuk array chunks.
pub fn simd_or(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len().max(b.len());
    if len < 8 {
        return scalar_or(a, b);
    }
    simd_dispatch_or(a, b, len)
}

/// SIMD-accelerated 4-state XOR untuk array chunks.
pub fn simd_xor(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len().max(b.len());
    if len < 8 {
        return scalar_xor(a, b);
    }
    simd_dispatch_xor(a, b, len)
}

/// SIMD-accelerated 4-state NOT untuk array chunks.
pub fn simd_not(a: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len();
    if len < 8 {
        return scalar_not(a);
    }
    simd_dispatch_not(a, len)
}

// ─── Dispatch ───

fn simd_dispatch_and(a: &[(u64, u64)], b: &[(u64, u64)], len: usize) -> Vec<(u64, u64)> {
    let (ka, va) = deinterleave(a, len);
    let (kb, vb) = deinterleave(b, len);
    let mut ok = vec![0u64; len];
    let mut ov = vec![0u64; len];

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            unsafe {
                avx512_and(&ka, &va, &kb, &vb, &mut ok, &mut ov);
            }
        } else if is_x86_feature_detected!("avx2") {
            unsafe {
                avx2_and(&ka, &va, &kb, &vb, &mut ok, &mut ov);
            }
        } else {
            scalar_and_arrays(&ka, &va, &kb, &vb, &mut ok, &mut ov);
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_and_arrays(&ka, &va, &kb, &vb, &mut ok, &mut ov);
    }

    // SIM-11: mode X-dominan → bit X operand membatalkan hasil AND TAPI
    // identitas dominasi LRM §11.4.1 Tabel 11-20 tetap menang: `0 & x = 0`
    // (koreksi dgn arah value.rs — dulu `0 & X = X` melanggar LRM → X
    // menular tak terbatas thd input tak-ter-init). X hanya utk bit tanpa
    // penentu 0. Z tetap non-dominan (`0 & Z = 0`, LRM).
    if x_dominant_bitwise() {
        let xa: Vec<u64> = ka.iter().zip(&va).map(|(k, v)| !k & !v).collect();
        let xb: Vec<u64> = kb.iter().zip(&vb).map(|(k, v)| !k & !v).collect();
        // Known-zero mask per bit (known=1 & value=0) — penentu dominasi AND.
        let za: Vec<u64> = ka.iter().zip(&va).map(|(k, v)| *k & !*v).collect();
        let zb: Vec<u64> = kb.iter().zip(&vb).map(|(k, v)| *k & !*v).collect();
        for i in 0..len {
            let zmask = za[i] | zb[i];
            // Base formula `ka & kb` menghasilkan known=0 (X) utk `0 & X`
            // → set POSITIF: penentu 0 → hasil pasti 0 (LRM §11.4.1).
            ok[i] |= zmask;
            ov[i] &= !zmask;
            // Sisanya: X hanya utk bit tanpa penentu 0.
            let xm = (xa[i] | xb[i]) & !zmask;
            ok[i] &= !xm;
            ov[i] &= !xm;
        }
    }

    interleave(&ok, &ov)
}

fn simd_dispatch_or(a: &[(u64, u64)], b: &[(u64, u64)], len: usize) -> Vec<(u64, u64)> {
    let (ka, va) = deinterleave(a, len);
    let (kb, vb) = deinterleave(b, len);
    let mut ok = vec![0u64; len];
    let mut ov = vec![0u64; len];

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            unsafe {
                avx512_or(&ka, &va, &kb, &vb, &mut ok, &mut ov);
            }
        } else if is_x86_feature_detected!("avx2") {
            unsafe {
                avx2_or(&ka, &va, &kb, &vb, &mut ok, &mut ov);
            }
        } else {
            scalar_or_arrays(&ka, &va, &kb, &vb, &mut ok, &mut ov);
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_or_arrays(&ka, &va, &kb, &vb, &mut ok, &mut ov);
    }

    // SIM-11: mode X-dominan → `0 | X = X` (LRM §11.4.1 Tabel 11-21 memang
    // `0|x=x`) TAPI identitas dominasi `1 | x = 1` tetap menang (koreksi
    // dgn arah value.rs — dulu `1 | X = X` melanggar LRM). X hanya utk bit
    // tanpa penentu 1. Z tetap non-dominan (`1 | Z = 1`, LRM).
    if x_dominant_bitwise() {
        let xa: Vec<u64> = ka.iter().zip(&va).map(|(k, v)| !k & !v).collect();
        let xb: Vec<u64> = kb.iter().zip(&vb).map(|(k, v)| !k & !v).collect();
        // Known-one mask per bit (known=1 & value=1) — penentu dominasi OR.
        let oa: Vec<u64> = ka.iter().zip(&va).map(|(k, v)| *k & *v).collect();
        let ob: Vec<u64> = kb.iter().zip(&vb).map(|(k, v)| *k & *v).collect();
        for i in 0..len {
            // X hanya utk bit TANPA penentu 1 — `1 | x = 1` tetap LRM.
            let xm = (xa[i] | xb[i]) & !(oa[i] | ob[i]);
            ok[i] &= !xm;
            ov[i] &= !xm;
        }
    }

    interleave(&ok, &ov)
}

fn simd_dispatch_xor(a: &[(u64, u64)], b: &[(u64, u64)], len: usize) -> Vec<(u64, u64)> {
    let (ka, va) = deinterleave(a, len);
    let (kb, vb) = deinterleave(b, len);
    let mut ok = vec![0u64; len];
    let mut ov = vec![0u64; len];

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            unsafe {
                avx512_xor(&ka, &va, &kb, &vb, &mut ok, &mut ov);
            }
        } else if is_x86_feature_detected!("avx2") {
            unsafe {
                avx2_xor(&ka, &va, &kb, &vb, &mut ok, &mut ov);
            }
        } else {
            scalar_xor_arrays(&ka, &va, &kb, &vb, &mut ok, &mut ov);
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_xor_arrays(&ka, &va, &kb, &vb, &mut ok, &mut ov);
    }

    interleave(&ok, &ov)
}

fn simd_dispatch_not(a: &[(u64, u64)], len: usize) -> Vec<(u64, u64)> {
    let (ka, va) = deinterleave(a, len);
    let mut ok = vec![0u64; len];
    let mut ov = vec![0u64; len];

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            unsafe {
                avx512_not(&ka, &va, &mut ok, &mut ov);
            }
        } else if is_x86_feature_detected!("avx2") {
            unsafe {
                avx2_not(&ka, &va, &mut ok, &mut ov);
            }
        } else {
            scalar_not_arrays(&ka, &va, &mut ok, &mut ov);
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        scalar_not_arrays(&ka, &va, &mut ok, &mut ov);
    }

    interleave(&ok, &ov)
}

// ─── AVX-512 (512-bit = 8×u64) ───

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_and(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    out_k: &mut [u64],
    out_v: &mut [u64],
) {
    let len = ka.len();
    let mut i = 0;
    while i + 8 <= len {
        let vka = _mm512_loadu_si512(ka.as_ptr().add(i) as *const __m512i);
        let vva = _mm512_loadu_si512(va.as_ptr().add(i) as *const __m512i);
        let vkb = _mm512_loadu_si512(kb.as_ptr().add(i) as *const __m512i);
        let vvb = _mm512_loadu_si512(vb.as_ptr().add(i) as *const __m512i);

        let a0 = _mm512_andnot_si512(vva, vka);
        let b0 = _mm512_andnot_si512(vvb, vkb);
        let a1 = _mm512_and_si512(vka, vva);
        let b1 = _mm512_and_si512(vkb, vvb);

        let known = _mm512_or_si512(_mm512_or_si512(a0, b0), _mm512_and_si512(a1, b1));
        let value = _mm512_and_si512(a1, b1);

        _mm512_storeu_si512(out_k.as_mut_ptr().add(i) as *mut __m512i, known);
        _mm512_storeu_si512(out_v.as_mut_ptr().add(i) as *mut __m512i, value);
        i += 8;
    }
    for j in i..len {
        let a_is_0 = ka[j] & !va[j];
        let b_is_0 = kb[j] & !vb[j];
        let a_is_1 = ka[j] & va[j];
        let b_is_1 = kb[j] & vb[j];
        out_k[j] = a_is_0 | b_is_0 | (a_is_1 & b_is_1);
        out_v[j] = a_is_1 & b_is_1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_or(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    out_k: &mut [u64],
    out_v: &mut [u64],
) {
    let len = ka.len();
    let mut i = 0;
    while i + 8 <= len {
        let vka = _mm512_loadu_si512(ka.as_ptr().add(i) as *const __m512i);
        let vva = _mm512_loadu_si512(va.as_ptr().add(i) as *const __m512i);
        let vkb = _mm512_loadu_si512(kb.as_ptr().add(i) as *const __m512i);
        let vvb = _mm512_loadu_si512(vb.as_ptr().add(i) as *const __m512i);

        let a0 = _mm512_andnot_si512(vva, vka);
        let b0 = _mm512_andnot_si512(vvb, vkb);
        let a1 = _mm512_and_si512(vka, vva);
        let b1 = _mm512_and_si512(vkb, vvb);

        let known = _mm512_or_si512(_mm512_or_si512(a1, b1), _mm512_and_si512(a0, b0));
        let value = _mm512_or_si512(a1, b1);

        _mm512_storeu_si512(out_k.as_mut_ptr().add(i) as *mut __m512i, known);
        _mm512_storeu_si512(out_v.as_mut_ptr().add(i) as *mut __m512i, value);
        i += 8;
    }
    for j in i..len {
        let a_is_0 = ka[j] & !va[j];
        let b_is_0 = kb[j] & !vb[j];
        let a_is_1 = ka[j] & va[j];
        let b_is_1 = kb[j] & vb[j];
        out_k[j] = a_is_1 | b_is_1 | (a_is_0 & b_is_0);
        out_v[j] = a_is_1 | b_is_1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_xor(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    out_k: &mut [u64],
    out_v: &mut [u64],
) {
    let len = ka.len();
    let mut i = 0;
    while i + 8 <= len {
        let vka = _mm512_loadu_si512(ka.as_ptr().add(i) as *const __m512i);
        let vva = _mm512_loadu_si512(va.as_ptr().add(i) as *const __m512i);
        let vkb = _mm512_loadu_si512(kb.as_ptr().add(i) as *const __m512i);
        let vvb = _mm512_loadu_si512(vb.as_ptr().add(i) as *const __m512i);

        let known = _mm512_and_si512(vka, vkb);
        let value = _mm512_and_si512(_mm512_xor_si512(vva, vvb), known);

        _mm512_storeu_si512(out_k.as_mut_ptr().add(i) as *mut __m512i, known);
        _mm512_storeu_si512(out_v.as_mut_ptr().add(i) as *mut __m512i, value);
        i += 8;
    }
    for j in i..len {
        out_k[j] = ka[j] & kb[j];
        out_v[j] = (va[j] ^ vb[j]) & out_k[j];
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_not(ka: &[u64], va: &[u64], out_k: &mut [u64], out_v: &mut [u64]) {
    let len = ka.len();
    let mut i = 0;
    while i + 8 <= len {
        let vka = _mm512_loadu_si512(ka.as_ptr().add(i) as *const __m512i);
        let vva = _mm512_loadu_si512(va.as_ptr().add(i) as *const __m512i);

        // NOT: known unchanged, value = !value & known
        let value = _mm512_andnot_si512(vva, vka);

        _mm512_storeu_si512(out_k.as_mut_ptr().add(i) as *mut __m512i, vka);
        _mm512_storeu_si512(out_v.as_mut_ptr().add(i) as *mut __m512i, value);
        i += 8;
    }
    for j in i..len {
        out_k[j] = ka[j];
        out_v[j] = (!va[j]) & ka[j];
    }
}

// ─── AVX2 (256-bit = 4×u64) ───

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_and(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    out_k: &mut [u64],
    out_v: &mut [u64],
) {
    let len = ka.len();
    let mut i = 0;
    while i + 4 <= len {
        let vka = _mm256_loadu_si256(ka.as_ptr().add(i) as *const __m256i);
        let vva = _mm256_loadu_si256(va.as_ptr().add(i) as *const __m256i);
        let vkb = _mm256_loadu_si256(kb.as_ptr().add(i) as *const __m256i);
        let vvb = _mm256_loadu_si256(vb.as_ptr().add(i) as *const __m256i);

        let a0 = _mm256_andnot_si256(vva, vka);
        let b0 = _mm256_andnot_si256(vvb, vkb);
        let a1 = _mm256_and_si256(vka, vva);
        let b1 = _mm256_and_si256(vkb, vvb);

        let known = _mm256_or_si256(_mm256_or_si256(a0, b0), _mm256_and_si256(a1, b1));
        let value = _mm256_and_si256(a1, b1);

        _mm256_storeu_si256(out_k.as_mut_ptr().add(i) as *mut __m256i, known);
        _mm256_storeu_si256(out_v.as_mut_ptr().add(i) as *mut __m256i, value);
        i += 4;
    }
    for j in i..len {
        let a_is_0 = ka[j] & !va[j];
        let b_is_0 = kb[j] & !vb[j];
        let a_is_1 = ka[j] & va[j];
        let b_is_1 = kb[j] & vb[j];
        out_k[j] = a_is_0 | b_is_0 | (a_is_1 & b_is_1);
        out_v[j] = a_is_1 & b_is_1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_or(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    out_k: &mut [u64],
    out_v: &mut [u64],
) {
    let len = ka.len();
    let mut i = 0;
    while i + 4 <= len {
        let vka = _mm256_loadu_si256(ka.as_ptr().add(i) as *const __m256i);
        let vva = _mm256_loadu_si256(va.as_ptr().add(i) as *const __m256i);
        let vkb = _mm256_loadu_si256(kb.as_ptr().add(i) as *const __m256i);
        let vvb = _mm256_loadu_si256(vb.as_ptr().add(i) as *const __m256i);

        let a0 = _mm256_andnot_si256(vva, vka);
        let b0 = _mm256_andnot_si256(vvb, vkb);
        let a1 = _mm256_and_si256(vka, vva);
        let b1 = _mm256_and_si256(vkb, vvb);

        let known = _mm256_or_si256(_mm256_or_si256(a1, b1), _mm256_and_si256(a0, b0));
        let value = _mm256_or_si256(a1, b1);

        _mm256_storeu_si256(out_k.as_mut_ptr().add(i) as *mut __m256i, known);
        _mm256_storeu_si256(out_v.as_mut_ptr().add(i) as *mut __m256i, value);
        i += 4;
    }
    for j in i..len {
        let a_is_0 = ka[j] & !va[j];
        let b_is_0 = kb[j] & !vb[j];
        let a_is_1 = ka[j] & va[j];
        let b_is_1 = kb[j] & vb[j];
        out_k[j] = a_is_1 | b_is_1 | (a_is_0 & b_is_0);
        out_v[j] = a_is_1 | b_is_1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_xor(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    out_k: &mut [u64],
    out_v: &mut [u64],
) {
    let len = ka.len();
    let mut i = 0;
    while i + 4 <= len {
        let vka = _mm256_loadu_si256(ka.as_ptr().add(i) as *const __m256i);
        let vva = _mm256_loadu_si256(va.as_ptr().add(i) as *const __m256i);
        let vkb = _mm256_loadu_si256(kb.as_ptr().add(i) as *const __m256i);
        let vvb = _mm256_loadu_si256(vb.as_ptr().add(i) as *const __m256i);

        let known = _mm256_and_si256(vka, vkb);
        let value = _mm256_and_si256(_mm256_xor_si256(vva, vvb), known);

        _mm256_storeu_si256(out_k.as_mut_ptr().add(i) as *mut __m256i, known);
        _mm256_storeu_si256(out_v.as_mut_ptr().add(i) as *mut __m256i, value);
        i += 4;
    }
    for j in i..len {
        out_k[j] = ka[j] & kb[j];
        out_v[j] = (va[j] ^ vb[j]) & out_k[j];
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2_not(ka: &[u64], va: &[u64], out_k: &mut [u64], out_v: &mut [u64]) {
    let len = ka.len();
    let mut i = 0;
    while i + 4 <= len {
        let vka = _mm256_loadu_si256(ka.as_ptr().add(i) as *const __m256i);
        let vva = _mm256_loadu_si256(va.as_ptr().add(i) as *const __m256i);

        let value = _mm256_andnot_si256(vva, vka);

        _mm256_storeu_si256(out_k.as_mut_ptr().add(i) as *mut __m256i, vka);
        _mm256_storeu_si256(out_v.as_mut_ptr().add(i) as *mut __m256i, value);
        i += 4;
    }
    for j in i..len {
        out_k[j] = ka[j];
        out_v[j] = (!va[j]) & ka[j];
    }
}

// ─── Scalar Fallbacks ───

fn scalar_and(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len().max(b.len());
    let pessimistic = x_dominant_bitwise();
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let (ak, av) = a.get(i).copied().unwrap_or((0, 0));
        let (bk, bv) = b.get(i).copied().unwrap_or((0, 0));
        let a0 = ak & !av;
        let b0 = bk & !bv;
        let a1 = ak & av;
        let b1 = bk & bv;
        let known = a0 | b0 | (a1 & b1);
        // SIM-11: mode X-dominan → X membatalkan dominasi TAPI identitas
        // LRM §11.4.1 Tabel 11-20 tetap menang: `0 & x = 0` (koreksi —
        // dulu `0 & X = X` melanggar LRM). X hanya utk bit tanpa penentu 0.
        // Z tetap non-dominan (`0 & Z = 0`, LRM) — xbit hanya true utk X
        // (known=0, value=0), bukan Z (known=0, value=1).
        let (known, val) = if pessimistic {
            let xbit = (!ak & !av) | (!bk & !bv);
            let zmask = a0 | b0; // penentu 0 → LRM menang
            let xm = xbit & !zmask;
            (known & !xm, (a1 & b1) & !xm)
        } else {
            (known, a1 & b1)
        };
        out.push((known, val));
    }
    out
}

fn scalar_or(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len().max(b.len());
    let pessimistic = x_dominant_bitwise();
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let (ak, av) = a.get(i).copied().unwrap_or((0, 0));
        let (bk, bv) = b.get(i).copied().unwrap_or((0, 0));
        let a0 = ak & !av;
        let b0 = bk & !bv;
        let a1 = ak & av;
        let b1 = bk & bv;
        let known = a1 | b1 | (a0 & b0);
        // SIM-11: X membatalkan dominasi TAPI identitas LRM §11.4.1 Tabel
        // 11-21 tetap menang: `1 | x = 1` (koreksi — dulu `1 | X = X`
        // melanggar LRM). `0 | X = X` tetap (LRM juga `0|x=x`).
        let (known, val) = if pessimistic {
            let xbit = (!ak & !av) | (!bk & !bv);
            let one_mask = a1 | b1; // penentu 1 → LRM menang
            let xm = xbit & !one_mask;
            (known & !xm, (a1 | b1) & !xm)
        } else {
            (known, a1 | b1)
        };
        out.push((known, val));
    }
    out
}

fn scalar_xor(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let len = a.len().max(b.len());
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let (ak, av) = a.get(i).copied().unwrap_or((0, 0));
        let (bk, bv) = b.get(i).copied().unwrap_or((0, 0));
        let known = ak & bk;
        out.push((known, (av ^ bv) & known));
    }
    out
}

fn scalar_not(a: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut out = Vec::with_capacity(a.len());
    for &(known, value) in a {
        out.push((known, (!value) & known));
    }
    out
}

// ─── Helpers ───

fn deinterleave(pairs: &[(u64, u64)], len: usize) -> (Vec<u64>, Vec<u64>) {
    let mut k = vec![0u64; len];
    let mut v = vec![0u64; len];
    for (i, &(ki, vi)) in pairs.iter().enumerate() {
        k[i] = ki;
        v[i] = vi;
    }
    (k, v)
}

fn interleave(k: &[u64], v: &[u64]) -> Vec<(u64, u64)> {
    let mut out = Vec::with_capacity(k.len());
    for i in 0..k.len() {
        out.push((k[i], v[i]));
    }
    out
}

// ─── Scalar Array Ops (untuk SIMD fallback) ───

fn scalar_and_arrays(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    ok: &mut [u64],
    ov: &mut [u64],
) {
    for i in 0..ka.len() {
        let a0 = ka[i] & !va[i];
        let b0 = kb[i] & !vb[i];
        let a1 = ka[i] & va[i];
        let b1 = kb[i] & vb[i];
        ok[i] = a0 | b0 | (a1 & b1);
        ov[i] = a1 & b1;
    }
}

fn scalar_or_arrays(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    ok: &mut [u64],
    ov: &mut [u64],
) {
    for i in 0..ka.len() {
        let a0 = ka[i] & !va[i];
        let b0 = kb[i] & !vb[i];
        let a1 = ka[i] & va[i];
        let b1 = kb[i] & vb[i];
        ok[i] = a1 | b1 | (a0 & b0);
        ov[i] = a1 | b1;
    }
}

fn scalar_xor_arrays(
    ka: &[u64],
    va: &[u64],
    kb: &[u64],
    vb: &[u64],
    ok: &mut [u64],
    ov: &mut [u64],
) {
    for i in 0..ka.len() {
        ok[i] = ka[i] & kb[i];
        ov[i] = (va[i] ^ vb[i]) & ok[i];
    }
}

fn scalar_not_arrays(ka: &[u64], va: &[u64], ok: &mut [u64], ov: &mut [u64]) {
    for i in 0..ka.len() {
        ok[i] = ka[i];
        ov[i] = (!va[i]) & ka[i];
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunks(values: &[(u64, u64)]) -> Vec<(u64, u64)> {
        values.to_vec()
    }

    // ─── AND Tests ───

    #[test]
    fn test_simd_and_small() {
        let a = make_chunks(&[(0xFF, 0xFF)]); // 11111111 (all known 1s)
        let b = make_chunks(&[(0xFF, 0x0F)]); // 00001111 (lower 4=1, upper 4=0)
        let r = simd_and(&a, &b);
        assert_eq!(r.len(), 1);
        // AND: 1&1=1, 1&0=0 — both inputs fully known, so all result bits known
        // known=0xFF (all bits determined), value=0x0F (lower 4 bits = 1)
        assert_eq!(r[0], (0xFF, 0x0F));
        assert_eq!(r[0].0 & r[0].1, 0x0F); // to_u64 equivalent
    }

    #[test]
    fn test_simd_and_with_x() {
        let a = make_chunks(&[(0, 0)]); // XXXXXXXX
        let b = make_chunks(&[(0xFF, 0xFF)]); // 11111111
        let r = simd_and(&a, &b);
        // X AND 1 = X → known=0, value=0
        assert_eq!(r[0], (0, 0));
    }

    #[test]
    fn test_simd_and_zero_dominates() {
        let a = make_chunks(&[(0xFF, 0x00)]); // 00000000
        let b = make_chunks(&[(0, 0)]); // XXXXXXXX
        let r = simd_and(&a, &b);
        // LRM §11.4.1 Tabel 11-20 (koreksi X-prop): 0 mendominasi AND →
        // `0 & X = 0` (dulu mode pessimistic membatalkan dominasi → X,
        // melanggar LRM → X menular thd input tak-ter-init).
        assert_eq!(r[0], (0xFF, 0x00));
    }

    #[test]
    fn test_simd_and_zero_dominates_optimistic() {
        let a = make_chunks(&[(0xFF, 0x00)]); // 00000000
        let b = make_chunks(&[(0, 0)]); // XXXXXXXX
        crate::simulator::value::set_xprop_mode(
            crate::simulator::types::XPropagationMode::Optimistic,
        );
        let r = simd_and(&a, &b);
        // Optimistic/LRM: 0 mendominasi AND → 0 AND X = 0
        assert_eq!(r[0], (0xFF, 0x00));
    }

    #[test]
    fn test_simd_and_multi_chunk() {
        let chunks = 16; // 1024 bit signal — triggers SIMD path (len >= 8)
        let a = make_chunks(&vec![(!0u64, !0u64); chunks]); // all 1s
        let b = make_chunks(&vec![(!0u64, 0xAAAAAAAAAAAAAAAA); chunks]); // alternating bits
        let r = simd_and(&a, &b);
        assert_eq!(r.len(), chunks);
        // 1 AND alternating = alternating, all bits known
        for &(known, value) in &r {
            assert_eq!(known, !0u64, "all bits should be known");
            assert_eq!(value, 0xAAAAAAAAAAAAAAAA, "bits should match b's pattern");
        }
    }

    #[test]
    fn test_simd_and_unequal_length() {
        let a = make_chunks(&[(0xFF, 0xFF), (0xFF, 0xFF)]);
        let b = make_chunks(&[(0xFF, 0x0F)]);
        let r = simd_and(&a, &b);
        assert_eq!(r.len(), 2);
        // Chunk 0: a[0] AND b[0] = (0xFF, 0x0F)
        assert_eq!(r[0], (0xFF, 0x0F));
        // Chunk 1: a[1] AND 0 (b padded to 0) = (0, 0) all X
        assert_eq!(r[1], (0, 0), "chunk with zero-padded input should be X");
    }

    // ─── OR Tests ───

    #[test]
    fn test_simd_or_basic() {
        let a = make_chunks(&[(0xFF, 0x0F)]);
        let b = make_chunks(&[(0xFF, 0xF0)]);
        let r = simd_or(&a, &b);
        assert_eq!(r[0], (0xFF, 0xFF)); // 0F | F0 = FF
    }

    // ─── XOR Tests ───

    #[test]
    fn test_simd_xor_basic() {
        let a = make_chunks(&[(0xFF, 0xAA)]);
        let b = make_chunks(&[(0xFF, 0x55)]);
        let r = simd_xor(&a, &b);
        assert_eq!(r[0], (0xFF, 0xFF)); // AA xor 55 = FF
    }

    // ─── NOT Test ───

    #[test]
    fn test_simd_not_basic() {
        let a = make_chunks(&[(0xFF, 0xAA)]);
        let r = simd_not(&a);
        // NOT AA = 55, known unchanged
        assert_eq!(r[0], (0xFF, 0x55));
    }

    #[test]
    fn test_simd_not_x() {
        let a = make_chunks(&[(0, 0)]); // all X
        let r = simd_not(&a);
        // NOT X = X
        assert_eq!(r[0], (0, 0));
    }

    // ─── Large Signal Tests ───

    #[test]
    fn test_simd_and_large_all_x() {
        let chunks = 64; // 4096 bits
        let a = make_chunks(&vec![(0u64, 0u64); chunks]);
        let b = make_chunks(&vec![(!0u64, !0u64); chunks]);
        let r = simd_and(&a, &b);
        // X AND 1 = X → known=0, value=0
        assert_eq!(r.len(), chunks);
        assert!(r.iter().all(|&(k, v)| k == 0 && v == 0));
    }

    #[test]
    fn test_simd_or_large_mixed() {
        let chunks = 32;
        let a = make_chunks(&vec![(0xFF, 0x00); chunks]); // all 0
        let b = make_chunks(&vec![(0xFF, 0xFF); chunks]); // all 1
        let r = simd_or(&a, &b);
        // 0 OR 1 = 1
        assert!(r.iter().all(|&(k, v)| k == 0xFF && v == 0xFF));
    }

    #[test]
    fn test_simd_xor_same_values() {
        let chunks = 16;
        let a = make_chunks(&vec![(!0u64, 0xDEADBEEF); chunks]);
        let b = make_chunks(&vec![(!0u64, 0xDEADBEEF); chunks]);
        let r = simd_xor(&a, &b);
        // X XOR X = 0 (known stays)
        assert!(r.iter().all(|&(k, v)| k == !0u64 && v == 0));
    }

    #[test]
    fn test_scalar_fallback_vs_simd() {
        // Verify scalar and SIMD produce identical results for random data
        let chunks = 20; // Not aligned to 4 or 8, tests tail handling
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let a: Vec<(u64, u64)> = (0..chunks).map(|_| (rng.gen(), rng.gen())).collect();
        let b: Vec<(u64, u64)> = (0..chunks).map(|_| (rng.gen(), rng.gen())).collect();

        let scalar_r = scalar_and(&a, &b);
        let simd_r = simd_and(&a, &b);
        assert_eq!(scalar_r, simd_r, "AND results must match");

        let scalar_r = scalar_or(&a, &b);
        let simd_r = simd_or(&a, &b);
        assert_eq!(scalar_r, simd_r, "OR results must match");

        let scalar_r = scalar_xor(&a, &b);
        let simd_r = simd_xor(&a, &b);
        assert_eq!(scalar_r, simd_r, "XOR results must match");

        let scalar_r = scalar_not(&a);
        let simd_r = simd_not(&a);
        assert_eq!(scalar_r, simd_r, "NOT results must match");
    }
}
