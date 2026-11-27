//! The interner: one id per text however the table grows, and member ids
//! no lookup returns.
use super::*;

#[test]
fn a_text_keeps_its_id_as_the_table_grows() {
    let mut strings = Interner::default();
    let texts: Vec<String> = (0..5000).map(|index| format!("text {index}")).collect();
    let mut ids = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        ids.push(strings.intern(text));
        // Every earlier text still finds its id, across each rehash.
        let earlier = &texts[index / 2];
        assert_eq!(strings.find(earlier), ids[index / 2]);
    }
    for (text, id) in texts.iter().zip(&ids) {
        assert_eq!(strings.intern(text), *id);
        let id = id.ok_or("no id");
        assert_eq!(id.map(|id| strings.get(id)), Ok(text.as_str()));
    }
    assert_eq!(strings.find("text 5000"), None);
    assert_eq!(strings.intern(""), strings.find(""));
    assert_eq!(std::mem::size_of::<Option<StrId>>(), 4);
}

#[test]
fn texts_alike_but_for_their_length_or_padding_differ() {
    let mut strings = Interner::default();
    let texts = [
        "a",
        "a\0",
        "a\0\0\0\0\0\0\0",
        "a\0\0\0\0\0\0\0\0",
        "abcdefgh",
        "abcdefgh\0",
    ];
    let ids: Vec<_> = texts.iter().map(|text| strings.intern(text)).collect();
    for (index, text) in texts.iter().enumerate() {
        assert_eq!(strings.find(text), ids[index]);
        assert!(ids[..index].iter().all(|earlier| *earlier != ids[index]));
    }
}

#[test]
fn a_member_id_and_name_are_read_back_but_never_found() -> Result<(), &'static str> {
    let mut strings = Interner::default();
    strings.reserve(4, 2, 64);
    let shape = strings.intern("ns#Shape").ok_or("no shape")?;
    let (id, name) = strings.push_member(shape, "Member").ok_or("no member")?;
    assert_eq!(
        (strings.get(id), strings.get(name)),
        ("ns#Shape$Member", "Member")
    );
    assert!(strings.is(name, "Member") && !strings.is(name, "Membe"));
    assert_eq!(strings.find("ns#Shape$Member"), None);
    assert_eq!(strings.find("Member"), None);
    // Interning either text gives it an id of its own.
    let interned = strings.intern("Member").ok_or("no text")?;
    assert!(interned != name && strings.get(interned) == "Member");
    let pool = strings.finish();
    assert_eq!(pool.get(id), "ns#Shape$Member");
    Ok(())
}
