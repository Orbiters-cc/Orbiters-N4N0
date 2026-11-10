//! A published state as parts that are shared, not copied (architecture review 2026-11-10, DRT-1).
//!
//! The tray service publishes its whole state on every change, to the window, the SteamVR overlay and the panels around
//! VRChat's menus. Held as one value it was copied whole for each publish, written whole as JSON to each of them, and
//! read whole again there, up to tens of times a second with N4N0's screen open, though a change is nearly always
//! one small thing.
//!
//! A [`Slice`] is one part of such a state behind a reference count:
//! - copying the state copies no part, it counts one more reader of each;
//! - changing a part (`state.part.field = …`, as before) first makes that part this copy's own, when another copy still
//!   reads it, so a state handed out never changes under its readers;
//! - two copies hold the same part exactly when nothing changed it between them, which [`Slice::same`] answers by
//!   comparing two addresses. That address is the part's version: there is no number to keep in step with it.
//!
//! [`Parts`] is a state made of such parts (and of small plain values, compared by value), which can say what differs
//! between two of its copies as JSON ([`changes_between`]) and take such a difference in ([`apply_changes`]): what a
//! link sends instead of the whole state. [`parts!`](crate::parts) writes it for a struct from the names of its fields.

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// One part of a published state, shared by every copy of the state made since it last changed.
pub struct Slice<T> {
    /// Named, not `.0`: a slice of a tuple reads the tuple's own `.0` through it.
    arc: Arc<T>,
}

impl<T> Slice<T> {
    pub fn new(value: T) -> Self {
        Self { arc: Arc::new(value) }
    }

    /// Whether the two are one and the same part: nothing changed it between the two states, known without looking at
    /// it. Two parts that are not the same may still be equal (`==` looks).
    pub fn same(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.arc, &b.arc)
    }
}

impl<T: PartialEq> Slice<T> {
    /// Sets the part to `value` unless it already is: a part set to what it was stays the same part. Returns whether it
    /// changed.
    pub fn set(&mut self, value: T) -> bool {
        let changed = *self.arc != value;
        if changed {
            self.arc = Arc::new(value);
        }
        changed
    }

    /// Becomes the same part as `previous` when the two are equal: what was touched and left as it was (a field set to
    /// the value it had, a list cleared that was empty) is then the same part again, and nobody downstream compares or
    /// sends it.
    pub fn settle(&mut self, previous: &Self) {
        if !Self::same(self, previous) && *self.arc == *previous.arc {
            self.arc = previous.arc.clone();
        }
    }
}

impl<T> Clone for Slice<T> {
    fn clone(&self) -> Self {
        Self { arc: self.arc.clone() }
    }
}

impl<T> Deref for Slice<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.arc
    }
}

/// Writing to a part makes it this copy's own first, when another copy of the state still reads it: one copy of the
/// part, at the first write after the state was handed out.
impl<T: Clone> DerefMut for Slice<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.arc)
    }
}

impl<T: PartialEq> PartialEq for Slice<T> {
    fn eq(&self, other: &Self) -> bool {
        Self::same(self, other) || *self.arc == *other.arc
    }
}

impl<T: PartialEq> PartialEq<T> for Slice<T> {
    fn eq(&self, other: &T) -> bool {
        *self.arc == *other
    }
}

impl<T: Eq> Eq for Slice<T> {}

impl<T: Default> Default for Slice<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Slice<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (*self.arc).fmt(f)
    }
}

impl<T> From<T> for Slice<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<A, T: FromIterator<A>> FromIterator<A> for Slice<T> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        Self::new(T::from_iter(iter))
    }
}

impl<'a, T> IntoIterator for &'a Slice<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&*self.arc).into_iter()
    }
}

/// As its value: a state with slices reads and writes the same JSON as one without.
impl<T: Serialize> Serialize for Slice<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (*self.arc).serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Slice<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::new)
    }
}

/// A state made of parts: [`Slice`]s, small values compared by value, and states of parts inside it. Written by
/// [`parts!`](crate::parts).
pub trait Parts {
    /// Adds to `out` every part that differs from `since`'s, under `prefix` and its name.
    fn write_changes(&self, since: &Self, prefix: &str, out: &mut Changes) -> serde_json::Result<()>;

    /// Takes the part `name` from `map`'s next value. `Ok(false)` when this state has no such part (the value is
    /// still in `map`).
    fn read_change<'de, A: MapAccess<'de>>(&mut self, name: &str, map: &mut A) -> Result<bool, A::Error>;

    /// [`Slice::settle`] for every slice of it, against the same state as it was last handed out.
    fn settle(&mut self, previous: &Self);
}

/// The parts that differ between two copies of a state, as a JSON object of each one's name and new value.
pub struct Changes {
    json: Vec<u8>,
    parts: usize,
}

impl Changes {
    fn new() -> Self {
        Self { json: vec![b'{'], parts: 0 }
    }

    /// One part's new value (called by [`Parts::write_changes`]). Names are fields' names, with a dot after each state
    /// they are inside: nothing JSON would have to escape.
    pub fn part<T: Serialize>(&mut self, prefix: &str, name: &str, value: &T) -> serde_json::Result<()> {
        debug_assert!(prefix.chars().chain(name.chars()).all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'), "{prefix}{name}");
        if self.parts > 0 {
            self.json.push(b',');
        }
        self.parts += 1;
        self.json.push(b'"');
        self.json.extend_from_slice(prefix.as_bytes());
        self.json.extend_from_slice(name.as_bytes());
        self.json.extend_from_slice(b"\":");
        serde_json::to_writer(&mut self.json, value)
    }
}

/// What differs in `now` from `since`, as a JSON object (`{}` when nothing does), and how many parts that is.
pub fn changes_between<T: Parts>(now: &T, since: &T) -> serde_json::Result<(Vec<u8>, usize)> {
    let mut changes = Changes::new();
    now.write_changes(since, "", &mut changes)?;
    changes.json.push(b'}');
    Ok((changes.json, changes.parts))
}

/// Takes into `state` what [`changes_between`] wrote. A name `state` has no part for is passed over, as an unknown
/// field of the whole state is. On an error `state` may hold some of the parts and not others: apply to a copy, and
/// keep the copy only when this succeeds.
pub fn apply_changes<T: Parts>(state: &mut T, json: &str) -> serde_json::Result<()> {
    struct Apply<'a, T>(&'a mut T);

    impl<'de, T: Parts> Visitor<'de> for Apply<'_, T> {
        type Value = ();

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an object of changed parts") // i18n: not shown (the wording of a line that could not be read; the client asks for the state again and tells nobody)
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            while let Some(name) = map.next_key::<String>()? {
                if !self.0.read_change(&name, &mut map)? {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }

    impl<'de, T: Parts> DeserializeSeed<'de> for Apply<'_, T> {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
            deserializer.deserialize_map(self)
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(json);
    Apply(state).deserialize(&mut deserializer)?;
    deserializer.end()
}

/// Makes a struct a state of parts ([`Parts`]): `plain` fields are compared by value and travel whole when they differ
/// (small things: a flag, a number, a short text); `slices` are its [`Slice`]s; `identity` are slices of values that
/// cannot be compared, which travel whenever they were written to, changed or not; `nested` fields are states of parts
/// themselves, whose parts travel one by one under the field's name and a dot. Every field must be in one of the
/// four: a field added to the struct and to none of them does not compile, so none can be forgotten on the wire.
///
/// ```
/// use orbiters_core::slice::Slice;
/// #[derive(Clone, Default)]
/// struct World { open: bool, people: Slice<Vec<String>> }
/// orbiters_core::parts!(World { plain: [open], slices: [people], identity: [], nested: [] });
/// ```
#[macro_export]
macro_rules! parts {
    ($state:ty { plain: [$($plain:ident),* $(,)?], slices: [$($slice:ident),* $(,)?], identity: [$($identity:ident),* $(,)?], nested: [$($nested:ident),* $(,)?] $(,)? }) => {
        impl $crate::slice::Parts for $state {
            fn write_changes(&self, since: &Self, prefix: &str, out: &mut $crate::slice::Changes) -> ::serde_json::Result<()> {
                // Every field, by name: one missing from the lists stops the build here.
                let Self { $($plain: _,)* $($slice: _,)* $($identity: _,)* $($nested: _,)* } = self;
                $(
                    if self.$plain != since.$plain {
                        out.part(prefix, stringify!($plain), &self.$plain)?;
                    }
                )*
                $(
                    if self.$slice != since.$slice {
                        out.part(prefix, stringify!($slice), &self.$slice)?;
                    }
                )*
                $(
                    if !$crate::slice::Slice::same(&self.$identity, &since.$identity) {
                        out.part(prefix, stringify!($identity), &self.$identity)?;
                    }
                )*
                $(
                    $crate::slice::Parts::write_changes(&self.$nested, &since.$nested, &format!("{prefix}{}.", stringify!($nested)), out)?;
                )*
                Ok(())
            }

            fn read_change<'de, A: ::serde::de::MapAccess<'de>>(&mut self, name: &str, map: &mut A) -> ::std::result::Result<bool, A::Error> {
                $(
                    if name == stringify!($plain) {
                        self.$plain = map.next_value()?;
                        return Ok(true);
                    }
                )*
                $(
                    if name == stringify!($slice) {
                        self.$slice = map.next_value()?;
                        return Ok(true);
                    }
                )*
                $(
                    if name == stringify!($identity) {
                        self.$identity = map.next_value()?;
                        return Ok(true);
                    }
                )*
                $(
                    if let Some(rest) = name.strip_prefix(concat!(stringify!($nested), ".")) {
                        return $crate::slice::Parts::read_change(&mut self.$nested, rest, map);
                    }
                )*
                let _ = map;
                Ok(false)
            }

            fn settle(&mut self, previous: &Self) {
                $(
                    $crate::slice::Slice::settle(&mut self.$slice, &previous.$slice);
                )*
                $(
                    $crate::slice::Parts::settle(&mut self.$nested, &previous.$nested);
                )*
                let _ = previous;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Inner {
        speed: f32,
        people: Slice<Vec<String>>,
    }
    crate::parts!(Inner { plain: [speed], slices: [people], identity: [], nested: [] });

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct State {
        open: bool,
        note: Option<String>,
        lines: Slice<Vec<String>>,
        settings: Slice<(u32, String)>,
        /// Something that cannot be compared.
        job: Slice<Job>,
        world: Inner,
    }
    crate::parts!(State { plain: [open, note], slices: [lines, settings], identity: [job], nested: [world] });

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Job(u32);

    fn busy() -> State {
        State {
            open: true,
            note: Some("a \"quoted\" note".into()),
            lines: vec!["one".to_string(), "two".to_string()].into(),
            settings: (7, "seven".to_string()).into(),
            job: Job(1).into(),
            world: Inner { speed: 1.25, people: vec!["Ada".to_string()].into() },
        }
    }

    fn changes(now: &State, since: &State) -> (String, usize) {
        let (json, parts) = changes_between(now, since).unwrap();
        (String::from_utf8(json).unwrap(), parts)
    }

    #[test]
    fn a_copy_shares_every_part_until_one_is_written() {
        let first = busy();
        let mut second = first.clone();
        assert!(Slice::same(&first.lines, &second.lines) && Slice::same(&first.settings, &second.settings) && Slice::same(&first.world.people, &second.world.people));
        // A write makes that part the writer's own; the copy handed out before keeps what it had.
        second.lines.push("three".into());
        assert!(!Slice::same(&first.lines, &second.lines));
        assert_eq!(*first.lines, ["one", "two"]);
        assert_eq!(*second.lines, ["one", "two", "three"]);
        assert!(Slice::same(&first.settings, &second.settings), "the parts not written stay shared");
        // Written again before anyone else reads it: the same allocation, no second copy.
        let own = &*second.lines as *const Vec<String>;
        second.lines.push("four".into());
        assert_eq!(own, &*second.lines as *const Vec<String>);
        // Reading through a mutable state copies nothing.
        let third = &mut second.clone();
        assert_eq!(third.lines.len(), 4);
        assert_eq!(third.settings.0, 7);
        assert!(Slice::same(&third.lines, &second.lines));
    }

    #[test]
    fn only_what_differs_travels_and_arrives_as_the_whole_state() {
        let first = busy();
        assert_eq!(changes(&first, &first), ("{}".into(), 0));
        let mut second = first.clone();
        second.world.speed = 1.5;
        assert_eq!(changes(&second, &first), (r#"{"world.speed":1.5}"#.into(), 1));
        second.note = None;
        second.settings.0 = 8;
        second.world.people.push("Grace".into());
        let (json, parts) = changes(&second, &first);
        assert_eq!(json, r#"{"note":null,"settings":[8,"seven"],"world.speed":1.5,"world.people":["Ada","Grace"]}"#);
        assert_eq!(parts, 4);
        // A client that held the first state holds the second after it, and what did not travel is still the part it had.
        let mut client = first.clone();
        apply_changes(&mut client, &json).unwrap();
        assert_eq!(client, second);
        assert!(Slice::same(&client.lines, &first.lines), "a part that did not travel is not replaced");
        assert!(!Slice::same(&client.settings, &first.settings));
        // Against a state with nothing in common, every part travels but the note, which is none in both: the whole
        // state arrives.
        let (json, parts) = changes(&second, &State::default());
        assert_eq!(parts, 6);
        let mut fresh = State::default();
        apply_changes(&mut fresh, &json).unwrap();
        assert_eq!(fresh, second);
    }

    #[test]
    fn a_part_touched_and_left_as_it_was_is_the_same_part_again() {
        let first = busy();
        let mut second = first.clone();
        // Written to, with what it held already: its own allocation now, equal to the one handed out.
        second.lines.retain(|_| true);
        second.world.people[0] = "Ada".into();
        assert!(!Slice::same(&second.lines, &first.lines) && second.lines == first.lines);
        assert_eq!(changes(&second, &first).1, 0, "equal parts do not travel, same or not");
        second.settle(&first);
        assert!(Slice::same(&second.lines, &first.lines) && Slice::same(&second.world.people, &first.world.people));
        // A part that is not compared travels as soon as it was written to, and stays its own.
        second.job.0 = 1;
        assert_eq!(changes(&second, &first), (r#"{"job":1}"#.into(), 1));
        second.settle(&first);
        assert!(!Slice::same(&second.job, &first.job));
        let mut second = first.clone();
        // One that did change keeps its own.
        second.lines.push("three".into());
        second.settle(&first);
        assert!(!Slice::same(&second.lines, &first.lines));
        // `set` leaves a part set to what it was alone.
        let mut third = second.clone();
        assert!(!third.settings.set((7, "seven".into())));
        assert!(Slice::same(&third.settings, &second.settings));
        assert!(third.settings.set((9, "nine".into())));
        assert_eq!(third.settings.0, 9);
        assert_eq!(second.settings.0, 7);
    }

    #[test]
    fn a_difference_that_cannot_be_read_says_so_and_unknown_parts_are_passed_over() {
        let mut state = busy();
        // A part this state does not have (a newer service): passed over, the rest taken.
        apply_changes(&mut state, r#"{"unheard_of":{"a":[1,2,{"b":null}]},"open":false,"world.unheard_of":3,"world.speed":2.0}"#).unwrap();
        assert!(!state.open);
        assert_eq!(state.world.speed, 2.0);
        // A value of the wrong kind, a cut line, something after the object: errors, never taken for "nothing changed".
        assert!(apply_changes(&mut state.clone(), r#"{"open":"yes"}"#).is_err());
        assert!(apply_changes(&mut state.clone(), r#"{"lines":["one""#).is_err());
        assert!(apply_changes(&mut state.clone(), r#"{"open":true} {"open":false}"#).is_err());
        assert!(apply_changes(&mut state.clone(), r#"[1,2]"#).is_err());
    }

    #[test]
    fn a_state_with_slices_is_the_same_json_as_one_without() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Plain {
            open: bool,
            note: Option<String>,
            lines: Vec<String>,
            settings: (u32, String),
            job: u32,
            world: PlainInner,
        }
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct PlainInner {
            speed: f32,
            people: Vec<String>,
        }
        let json = serde_json::to_string(&busy()).unwrap();
        let plain: Plain = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&plain).unwrap(), json);
        assert_eq!(serde_json::from_str::<State>(&json).unwrap(), busy());
        // Iterating and comparing with a plain value, as code written for the plain field does.
        let state = busy();
        assert_eq!(state.lines, vec!["one".to_string(), "two".to_string()]);
        assert_eq!((&state.lines).into_iter().count(), 2);
        let mut seen = Vec::new();
        for line in &state.lines {
            seen.push(line.as_str());
        }
        assert_eq!(seen, ["one", "two"]);
    }
}
