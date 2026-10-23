//! Name suggestion engine — "did you mean?" untuk identifier tak dikenal.
//!
//! PARSER-03: Edit distance (Levenshtein) untuk saran nama saat
//! identifier tidak ditemukan di elaborator/engine.

/// Levenshtein distance antara dua string. O(n*m) time, O(min(n,m)) space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();

    // Gunakan dua row saja (rolling buffer).
    let mut prev = Vec::with_capacity(b_len + 1);
    let mut curr = vec![0usize; b_len + 1];

    for j in 0..=b_len {
        prev.push(j);
    }

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            let del = prev[j] + 1;
            let ins = curr[j - 1] + 1;
            let sub = prev[j - 1] + cost;
            curr[j] = del.min(ins).min(sub);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_len]
}

/// Cari nama terdekat dari kandidat berdasarkan Levenshtein distance.
/// Mengembalikan (nama_saran, distance) atau None jika tidak ada kandidat
/// dengan distance <= max_dist.
///
/// Untuk nama panjang, threshold distanceProporsional:
/// - Nama ≤ 4 char: max distance 1
/// - Nama 5-8 char: max distance 2
/// - Nama > 8 char: max distance 3
pub fn suggest_name<'a>(
    target: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<(&'a str, usize)> {
    let max_dist = match target.len() {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    };

    let mut best: Option<(&str, usize)> = None;

    for cand in candidates {
        if cand == target {
            continue; // exact match → skip
        }
        // Quick pre-filter: jika panjang beda > max_dist, skip
        let len_diff = (cand.len() as isize - target.len() as isize).unsigned_abs();
        if len_diff > max_dist {
            continue;
        }
        let dist = levenshtein(target, cand);
        if dist <= max_dist {
            match best {
                None => best = Some((cand, dist)),
                Some((_, best_dist)) if dist < best_dist => best = Some((cand, dist)),
                Some(_) => {} // tie → pertahankan yang pertama
            }
        }
    }
    best
}

/// Format sugesti menjadi string "did you mean 'X'?" atau "" jika tidak ada.
pub fn format_suggestion(target: &str, candidates: impl Iterator<Item = String>) -> String {
    let cand_owned: Vec<String> = candidates.collect();
    let cand_refs: Vec<&str> = cand_owned.iter().map(|s| s.as_str()).collect();
    if let Some((suggested, _dist)) = suggest_name(target, cand_refs.into_iter()) {
        format!(" — did you mean '{}'?", suggested)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_one_edit() {
        assert_eq!(levenshtein("hello", "helo"), 1); // deletion
        assert_eq!(levenshtein("hello", "helllo"), 1); // insertion
        assert_eq!(levenshtein("hello", "hallo"), 1); // substitution
    }

    #[test]
    fn test_levenshtein_multiple() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn test_suggest_name_basic() {
        let candidates = ["clk", "rst_n", "data_in", "data_out", "valid"];
        // Exact match "clka" mirip "clk"
        let (suggested, dist) = suggest_name("clka", candidates.iter().copied()).unwrap();
        assert_eq!(suggested, "clk");
        assert_eq!(dist, 1); // insert 'a'
    }

    #[test]
    fn test_suggest_name_close() {
        let candidates = ["data_in", "data_out", "valid", "ready"];
        let (suggested, _) = suggest_name("data_oun", candidates.iter().copied()).unwrap();
        assert_eq!(suggested, "data_out");
    }

    #[test]
    fn test_suggest_name_no_match() {
        let candidates = ["clk", "rst_n"];
        let result = suggest_name("xyz_completely_different", candidates.iter().copied());
        assert!(result.is_none());
    }

    #[test]
    fn test_suggest_name_typo() {
        let candidates = ["counter", "config", "control", "clock"];
        let (suggested, _) = suggest_name("conter", candidates.iter().copied()).unwrap();
        assert_eq!(suggested, "counter");
    }

    #[test]
    fn test_format_suggestion_with_match() {
        let candidates = vec!["data_in".to_string(), "data_out".to_string()];
        let result = format_suggestion("data_oun", candidates.into_iter());
        assert!(result.contains("did you mean"));
        assert!(result.contains("data_out"));
    }

    #[test]
    fn test_format_suggestion_no_match() {
        let candidates = vec!["abc".to_string()];
        let result =
            format_suggestion("xyz_completely_different_very_long", candidates.into_iter());
        assert!(result.is_empty());
    }
}
