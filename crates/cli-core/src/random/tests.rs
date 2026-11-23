use super::*;

/// Successive words differ, and their bits are not all stuck.
#[test]
fn words_vary_and_use_every_bit() {
    let words: Vec<u64> = (0..256).map(|_| word()).collect();
    let mut unique = words.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), words.len());
    let ones = words.iter().fold(0, |seen, word| seen | word);
    let zeros = words.iter().fold(0, |seen, word| seen | !word);
    assert_eq!((ones, zeros), (u64::MAX, u64::MAX));
}
