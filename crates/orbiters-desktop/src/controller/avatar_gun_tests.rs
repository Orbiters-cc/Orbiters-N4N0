//! DCT-1, DCT-7: what a hit may still do by the time its change would leave, and which of several at once does it.

use super::*;
use orbiters_core::vrchat_api::{Routed, Sent};
use orbiters_core::vrchat_log::{LogEvent, WorldState};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};

const AVATAR: &str = "avtr_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
/// The credential the hits come to.
const THEIRS: &str = "theirs";
const WORLD: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
const FRIEND: &str = "usr_1339e525-d75d-4832-9104-98dd89287681";

struct Playing {
    ctl: Shared,
    to: Member,
    asked: Arc<Mutex<Vec<Sent>>>,
    instance: String,
}

/// In an instance, signed in to Orbiters and to VRChat; `asked` happens as VRChat is asked something, before it answers.
fn playing(allow: AvatarGunAllow, asked: impl Fn(&Shared, &str) + Send + Sync + 'static) -> Playing {
    let ctl = Controller::new(orbiters_core::settings::Settings::default());
    let weak: Weak<Controller> = Arc::downgrade(&ctl);
    let sent = ctl.test_vrchat_routed(Arc::new(move |sent| {
        let url = sent.url.as_str();
        if let Some(ctl) = weak.upgrade() {
            asked(&ctl, url);
        }
        Routed::Answer(
            200,
            if url.contains("/friends") {
                serde_json::json!([{ "id": FRIEND, "displayName": "Sample" }])
            } else if url.contains("/avatars/") {
                serde_json::json!({ "id": AVATAR, "name": "Orbit", "releaseStatus": "public" })
            } else {
                serde_json::json!({ "id": "usr_test", "displayName": "Me" })
            },
        )
    }));
    ctl.update_settings(|s| s.avatar_gun.allow = allow);
    let mut world = WorldState::default();
    world.apply(&LogEvent::Joining { world_id: WORLD.into(), instance: "1".into(), at: None });
    world.apply(&LogEvent::LocalPlayer { name: "Me".into() });
    world.apply(&LogEvent::PlayerJoined { name: "Me".into(), user_id: "usr_test".into() });
    let instance = world.instance_hash().unwrap();
    ctl.update(|s| s.vrchat.world.set(world));
    // (Nothing of it is sent in a test: `avatar_gun_answer`.)
    let to = Member { api: Api::at("http://127.0.0.1:9/"), token: THEIRS.into(), epoch: ctl.identity_epoch() };
    Playing { ctl, to, asked: sent, instance }
}

impl Playing {
    fn hit(&self, shot: &str) {
        let hit = gun::Hit { shot: shot.into(), avatar: gun::Avatar::for_shot(AVATAR, "Orbit", None).unwrap(), from: gun::From { name: "Sample".into(), vrchat_id: Some(FRIEND.into()) } };
        self.ctl.avatar_gun_hit(hit, &self.to, &self.instance);
    }
    fn selected(&self) -> bool {
        self.asked.lock().unwrap().iter().any(|s| s.method == "PUT")
    }
    fn answers(&self, shot: &str) -> Vec<Outcome> {
        self.ctl.avatar_gun.lock().unwrap().answered.iter().filter(|(s, ..)| s == shot).map(|(_, o, _)| *o).collect()
    }
    /// The hit was answered, and as `outcome`.
    fn ends(&self, shot: &str, outcome: Outcome) {
        assert!(until(|| self.answers(shot).last() == Some(&outcome)), "{:?}", self.answers(shot));
        assert!(self.ctl.avatar_gun.lock().unwrap().answered.iter().all(|(.., token)| token == THEIRS), "as the member it came to");
    }
}

fn leave(ctl: &Shared) {
    ctl.update(|s| {
        let mut world = (*s.vrchat.world).clone();
        world.apply(&LogEvent::LeftRoom);
        s.vrchat.world.set(world);
    });
}

fn until(what: impl Fn() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(30) {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// DCT-1: the room left, the setting tightened, or another account signed in while the shooter is looked up, the avatar read, or
/// VRChat asked who is signed in: the avatar is not changed, and the shooter is told no.
#[test]
fn what_allowed_a_hit_is_looked_at_again_as_its_change_leaves() {
    for name in ["left", "tightened", "another account"] {
        for at in ["/friends", "/avatars/", "/auth/user"] {
            let allow = if at == "/friends" { AvatarGunAllow::Friends } else { AvatarGunAllow::Anyone };
            let p = playing(allow, move |ctl, url| {
                let here = if at == "/auth/user" { url.ends_with(at) } else { url.contains(at) && !(at == "/avatars/" && url.ends_with("/select")) };
                if here {
                    match name {
                        "left" => leave(ctl),
                        "tightened" => ctl.update_settings(|s| s.avatar_gun.allow = AvatarGunAllow::Nobody),
                        _ => drop(ctl.identity_epoch.fetch_add(1, Ordering::SeqCst)),
                    }
                }
            });
            p.hit("shot");
            p.ends("shot", Outcome::Refused);
            assert!(!p.selected(), "{name}, {at}");
            assert!(until(|| p.ctl.avatar_gun.lock().unwrap().hit.is_none()), "{name}, {at}");
        }
    }
}

/// The person's own Allow stays, whatever the setting says since; not once they left the instance.
#[test]
fn what_the_person_allowed_stays_allowed() {
    for left in [false, true] {
        let p = playing(AvatarGunAllow::AskFirst, |_, _| {});
        p.hit("shot");
        p.ends("shot", Outcome::Asked);
        assert!(until(|| p.ctl.view().avatar_gun.card.is_some()));
        let card = p.ctl.view().avatar_gun.card.as_ref().unwrap().id;
        if left {
            leave(&p.ctl);
        }
        p.ctl.avatar_gun_command(Command::Main { card });
        if left {
            p.ends("shot", Outcome::Refused);
            assert!(!p.selected());
        } else {
            assert!(until(|| p.selected()));
        }
    }
}

/// DCT-1: VRChat did not follow its API, and the instance was left meanwhile: nothing goes over OSC.
#[test]
fn the_osc_way_is_not_taken_once_the_instance_is_left() {
    let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    for left in [true, false] {
        let p = playing(AvatarGunAllow::Anyone, move |ctl, url| {
            if left && url.ends_with("/select") {
                leave(ctl);
            }
        });
        p.ctl.update_settings(|s| s.osc.vrchat_port = vrchat.local_addr().unwrap().port());
        p.ctl.start_osc_with(false);
        p.hit("shot");
        p.ends("shot", Outcome::Refused);
        assert!(p.selected());
        let mut buf = [0u8; 1024];
        let mut sent = 0;
        while let Ok(n) = vrchat.recv(&mut buf) {
            sent += matches!(rosc::decoder::decode_udp(&buf[..n]), Ok((_, rosc::OscPacket::Message(m))) if m.addr == AVATAR_CHANGE) as usize;
        }
        assert_eq!(sent > 0, !left, "left: {left}");
        p.ctl.shutdown();
    }
}

/// DCT-7: of two hits at once, one is asked about or changes the avatar, the other is refused.
#[test]
fn one_of_two_hits_at_once_is_taken() {
    for round in 0..20 {
        let p = playing(AvatarGunAllow::AskFirst, |_, _| {});
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for n in 0..8 {
                let (p, start) = (&p, &start);
                scope.spawn(move || {
                    start.wait();
                    p.hit(&format!("shot {n}"));
                });
            }
        });
        let all = |outcome: Outcome| p.ctl.avatar_gun.lock().unwrap().answered.iter().filter(|(_, o, _)| *o == outcome).count();
        assert!(until(|| all(Outcome::Asked) + all(Outcome::Refused) == 8));
        assert_eq!(all(Outcome::Asked), 1, "round {round}");
        let card = p.ctl.view().avatar_gun.card.as_ref().map(|c| c.id);
        assert_eq!(card, p.ctl.avatar_gun.lock().unwrap().hit.as_ref().map(|h| h.card), "the card shown is the hit's, with its buttons");
    }
}

/// DCT-7: a change that fails late, its card gone, leaves the hit that came since.
#[test]
fn a_late_failure_leaves_the_hit_that_came_since() {
    let p = playing(AvatarGunAllow::Anyone, |ctl, url| {
        if url.contains("/avatars/") {
            // Its card's time is over, and another hit is under way.
            let card = ctl.avatar_gun.lock().unwrap().hit.as_ref().unwrap().card;
            ctl.avatar_gun_end(card);
            let mut rt = ctl.avatar_gun.lock().unwrap();
            rt.hit = Some(Hit {
                shot: "newer".into(),
                avatar: gun::Avatar::for_shot(AVATAR, "Orbit", None).unwrap(),
                from: "Sample".into(),
                card: card + 1,
                instance: String::new(),
                to: Member { api: Api::at("http://127.0.0.1:9/"), token: THEIRS.into(), epoch: 0 },
                friend: false,
                allowed: true,
            });
        }
    });
    p.hit("older");
    p.ends("older", Outcome::Refused);
    assert!(!p.selected(), "its claim is gone");
    assert_eq!(p.ctl.avatar_gun.lock().unwrap().hit.as_ref().map(|h| h.shot.clone()).as_deref(), Some("newer"));
}

/// DCT-23: how an event stream ended that another took the place of says nothing of when that one is tried again.
#[test]
fn a_replaced_streams_end_leaves_the_other_ones_waits() {
    let p = playing(AvatarGunAllow::Anyone, |_, _| {});
    for well in [true, false] {
        {
            let mut rt = p.ctl.avatar_gun.lock().unwrap();
            (rt.streaming, rt.connected, rt.failures, rt.retry_at) = (Some((2, "token".into(), "instance".into(), 1)), true, 3, None);
        }
        p.ctl.avatar_gun_stream_ended(1, well);
        let rt = p.ctl.avatar_gun.lock().unwrap();
        assert_eq!((rt.streaming.is_some(), rt.connected, rt.failures, rt.retry_at), (true, true, 3, None), "well: {well}");
        drop(rt);
        p.ctl.avatar_gun_stream_ended(2, well);
        let rt = p.ctl.avatar_gun.lock().unwrap();
        assert_eq!((rt.streaming.is_some(), rt.connected, rt.failures, rt.retry_at.is_some()), (false, false, if well { 0 } else { 4 }, true), "well: {well}");
    }
}
