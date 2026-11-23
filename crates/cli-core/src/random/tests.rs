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

/// The operating system's random source fills every byte asked for, and two
/// draws differ.
#[test]
fn the_operating_system_fills_random_bytes() -> std::io::Result<()> {
    let first = os_bytes::<64>()?;
    let second = os_bytes::<64>()?;
    assert_ne!(first, second);
    assert_ne!(first, [0; 64]);
    Ok(())
}
