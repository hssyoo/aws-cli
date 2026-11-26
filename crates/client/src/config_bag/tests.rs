use super::*;

#[derive(Debug, Default, PartialEq)]
struct Count(u32);

#[derive(Debug, Default, PartialEq)]
struct Label(&'static str);

#[test]
fn a_slot_is_made_from_its_default_once_per_type() {
    let mut slots = Slots::default();
    assert_eq!(slots.get::<Count>(), &Count(0));
    slots.get::<Count>().0 += 2;
    slots.get::<Count>().0 += 3;
    slots.get::<Label>().0 = "seen";
    assert_eq!(slots.find::<Count>(), Some(&Count(5)));
    assert_eq!(slots.find::<Label>(), Some(&Label("seen")));
    assert_eq!(format!("{slots:?}"), "Slots(2)");
}

#[test]
fn find_makes_nothing() {
    let mut slots = Slots::default();
    assert_eq!(slots.find::<Count>(), None);
    assert!(slots.is_empty());
    slots.get::<Label>();
    assert_eq!(slots.find::<Count>(), None);
    assert_eq!(slots.len(), 1);
}

#[test]
fn the_first_default_of_each_kind_wins() {
    let list = [
        SettingDefault::RetryBaseDelay(Duration::from_millis(25)),
        SettingDefault::MaxAttempts(5),
        SettingDefault::MaxAttempts(10),
        SettingDefault::RetryBaseDelay(Duration::from_millis(50)),
    ];
    let defaults = SettingDefaults::first_wins(&list);
    assert_eq!(defaults.max_attempts(None), Some(5));
    assert_eq!(defaults.retry_base_delay(), Some(Duration::from_millis(25)));
    // The user's value wins, whatever it is.
    assert_eq!(defaults.max_attempts(Some(0)), Some(0));
    assert_eq!(defaults.max_attempts(Some(9)), Some(9));
    let none = SettingDefaults::first_wins(&[]);
    assert_eq!(
        (none.max_attempts(None), none.retry_base_delay()),
        (None, None)
    );
}
