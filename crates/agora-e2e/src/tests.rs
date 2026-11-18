use super::*;

const CH: ChannelId = ChannelId(7);

/// The part of the server that matters here: one log, one epoch, one GroupInfo.
#[derive(Default)]
struct Server {
    log: Vec<Entry>,
    epoch: u64,
    info: Option<Vec<u8>>,
    next_message: i64,
}

impl Server {
    fn append(&mut self, author: UserId, message_id: Option<MessageId>, data: Vec<u8>) -> i64 {
        let seq = self.log.len() as i64 + 1;
        self.log.push(Entry {
            seq,
            author,
            message_id,
            data,
        });
        seq
    }

    fn commit(&mut self, author: UserId, p: &Pending) -> std::result::Result<i64, ()> {
        let h = inspect(&p.commit).unwrap();
        if h.epoch != self.epoch || h.kind != Kind::Commit || h.group_id != group_id(CH) {
            return Err(());
        }
        let (gid, epoch) = inspect_group_info(&p.group_info).unwrap();
        assert_eq!(gid, group_id(CH));
        assert_eq!(epoch, self.epoch + 1);
        self.epoch = epoch;
        self.info = Some(p.group_info.clone());
        Ok(self.append(author, None, p.commit.clone()))
    }

    fn post(&mut self, author: UserId, data: Vec<u8>) -> std::result::Result<(MessageId, i64), ()> {
        let h = inspect(&data).unwrap();
        if h.epoch != self.epoch || h.kind != Kind::Application {
            return Err(());
        }
        self.next_message += 1;
        let id = MessageId(self.next_message);
        Ok((id, self.append(author, Some(id), data)))
    }

    fn sync(&self, d: &mut Device) -> Vec<Outcome> {
        let from = d.last_seq(CH);
        self.log
            .iter()
            .filter(|e| e.seq > from)
            .map(|e| d.process(CH, e).unwrap())
            .collect()
    }

    fn enter(&mut self, d: &mut Device) {
        let from = self.log.len() as i64;
        match self.info.clone() {
            None => {
                self.info = Some(d.create(CH, from).unwrap());
                d.confirm(CH, None).unwrap();
            }
            Some(info) => {
                let p = d.join(CH, &info, from).unwrap();
                let seq = self.commit(d.user(), &p).unwrap();
                d.confirm(CH, Some(seq)).unwrap();
            }
        }
    }

    fn send(&mut self, d: &mut Device, body: &str) -> MessageId {
        let (data, _) = d.encrypt(CH, body, None).unwrap();
        let (id, seq) = self.post(d.user(), data).unwrap();
        d.sent(CH, id, Some(seq), body);
        id
    }
}

fn device(user: i64) -> Device {
    Device::new(UserId(user)).unwrap()
}

#[test]
fn two_devices_talk_and_the_log_holds_no_plaintext() {
    let mut server = Server::default();
    let (mut ana, mut ben) = (device(1), device(2));
    server.enter(&mut ana);
    server.sync(&mut ana);
    server.enter(&mut ben);
    assert!(
        matches!(server.sync(&mut ana).as_slice(), [Outcome::Changed { joined, .. }] if joined[0].user == UserId(2))
    );

    let id = server.send(&mut ana, "¿Quedamos a las 5? 🙂");
    let out: Vec<Outcome> = server
        .sync(&mut ben)
        .into_iter()
        .filter(|o| *o != Outcome::Skipped)
        .collect();
    assert_eq!(
        out,
        vec![Outcome::Message {
            message_id: id,
            body: "¿Quedamos a las 5? 🙂".into()
        }]
    );
    assert_eq!(ana.plaintext(id), Some("¿Quedamos a las 5? 🙂"));
    assert!(server.sync(&mut ana).iter().all(|o| *o == Outcome::Skipped));
    for e in &server.log {
        let text = String::from_utf8_lossy(&e.data);
        assert!(!text.contains("Quedamos"), "plaintext leaked into the log");
    }
    assert_eq!(ana.members(CH).len(), 2);
}

#[test]
fn a_device_that_joins_later_cannot_read_what_came_before() {
    let mut server = Server::default();
    let (mut ana, mut ben) = (device(1), device(2));
    server.enter(&mut ana);
    let early = server.send(&mut ana, "before Ben");
    server.enter(&mut ben);
    server.sync(&mut ana);
    let later = server.send(&mut ana, "after Ben");
    server.sync(&mut ben);
    assert_eq!(ben.plaintext(early), None);
    assert_eq!(ben.plaintext(later), Some("after Ben"));
}

#[test]
fn a_removed_device_reads_nothing_new() {
    let mut server = Server::default();
    let (mut ana, mut ben) = (device(1), device(2));
    server.enter(&mut ana);
    server.enter(&mut ben);
    server.sync(&mut ana);

    let p = ana
        .remove(CH, |leaf| leaf.user != UserId(2))
        .unwrap()
        .unwrap();
    let seq = server.commit(UserId(1), &p).unwrap();
    ana.confirm(CH, Some(seq)).unwrap();
    let secret = server.send(&mut ana, "only for Ana");

    // Even handed the whole log, Ben's device is out of the group.
    assert!(server.sync(&mut ben).contains(&Outcome::Removed));
    assert!(!ben.is_member(CH));
    assert_eq!(ben.plaintext(secret), None);
    assert_eq!(ana.remove(CH, |_| true).unwrap().map(|_| ()), None);
}

#[test]
fn a_stale_commit_is_refused_and_can_be_retried() {
    let mut server = Server::default();
    let (mut ana, mut ben, mut cleo) = (device(1), device(2), device(3));
    server.enter(&mut ana);
    let info = server.info.clone().unwrap();
    // Ben and Cleo both join from the same GroupInfo; only the first commit fits the epoch.
    let pb = ben.join(CH, &info, 0).unwrap();
    let pc = cleo.join(CH, &info, 0).unwrap();
    let sb = server.commit(UserId(2), &pb).unwrap();
    ben.confirm(CH, Some(sb)).unwrap();
    assert!(server.commit(UserId(3), &pc).is_err());
    cleo.abort(CH).unwrap();
    assert!(!cleo.is_member(CH));
    server.enter(&mut cleo);
    server.sync(&mut ana);
    server.sync(&mut ben);
    let id = server.send(&mut cleo, "hola");
    assert_eq!(
        server.sync(&mut ana).last(),
        Some(&Outcome::Message {
            message_id: id,
            body: "hola".into()
        })
    );
    assert_eq!(
        server.sync(&mut ben).last(),
        Some(&Outcome::Message {
            message_id: id,
            body: "hola".into()
        })
    );
}

#[test]
fn a_saved_device_picks_up_where_it_stopped() {
    let mut server = Server::default();
    let (mut ana, mut ben) = (device(1), device(2));
    server.enter(&mut ana);
    server.enter(&mut ben);
    server.sync(&mut ana);
    let first = server.send(&mut ana, "one");
    server.sync(&mut ben);

    let mut ben = Device::from_bytes(&ben.to_bytes().unwrap()).unwrap();
    assert_eq!(ben.plaintext(first), Some("one"));
    let second = server.send(&mut ana, "two");
    server.sync(&mut ben);
    assert_eq!(ben.plaintext(second), Some("two"));
    assert_eq!(ben.search("TW", 10), vec![second]);
}

#[test]
fn an_edit_and_a_forged_author_are_checked() {
    let mut server = Server::default();
    let (mut ana, mut ben) = (device(1), device(2));
    server.enter(&mut ana);
    server.enter(&mut ben);
    server.sync(&mut ana);
    let id = server.send(&mut ana, "draft");
    let (edit, _) = ana.encrypt(CH, "final", Some(id)).unwrap();
    server.append(UserId(1), Some(id), edit);
    server.sync(&mut ben);
    assert_eq!(ben.plaintext(id), Some("final"));

    // A server that claims Ben wrote Ana's message gets an unreadable entry, not a lie.
    let (data, _) = ana.encrypt(CH, "mine", None).unwrap();
    server.append(UserId(2), Some(MessageId(99)), data);
    assert_eq!(server.sync(&mut ben), vec![Outcome::Unreadable]);
    assert_eq!(ben.plaintext(MessageId(99)), None);
}

#[test]
fn garbage_is_refused_without_panicking() {
    assert!(inspect(b"").is_err());
    assert!(inspect(&[0xff; 64]).is_err());
    assert!(inspect_group_info(b"nope").is_err());
    assert!(Device::from_bytes(b"{}").is_err());
    let mut d = device(1);
    assert!(d.join(CH, b"junk", 0).is_err());
    let out = d
        .process(
            CH,
            &Entry {
                seq: 1,
                author: UserId(2),
                message_id: None,
                data: vec![1, 2, 3],
            },
        )
        .unwrap();
    assert_eq!(out, Outcome::Unreadable);
    assert_eq!(fingerprint(b"k").split(' ').count(), 6);
}

#[test]
fn an_offline_device_is_added_with_its_key_package_and_reads_from_then_on() {
    let mut server = Server::default();
    let (mut ana, mut ben) = (device(1), device(2));
    server.enter(&mut ana);
    let before = server.send(&mut ana, "before Ben was added");
    let kp = ben.key_packages(2).unwrap();
    let ben_leaf = Leaf {
        user: UserId(2),
        public_key: ben.public_key(),
    };
    // A key package listed under someone else is refused.
    let wrong = Leaf {
        user: UserId(3),
        public_key: ben.public_key(),
    };
    assert!(ana.add(CH, &[(wrong, kp[1].clone())]).unwrap().is_none());

    let (p, welcome) = ana.add(CH, &[(ben_leaf, kp[0].clone())]).unwrap().unwrap();
    let seq = server.commit(UserId(1), &p).unwrap();
    ana.confirm(CH, Some(seq)).unwrap();
    let after = server.send(&mut ana, "while Ben was away");

    // Ben comes online later, enters through the Welcome and catches up.
    ben.welcome(CH, &welcome, seq).unwrap();
    server.sync(&mut ben);
    assert_eq!(ben.plaintext(before), None);
    assert_eq!(ben.plaintext(after), Some("while Ben was away"));
    assert_eq!(ana.members(CH).len(), 2);
}

#[test]
fn a_call_key_is_shared_by_the_members_and_moves_on_without_a_removed_device() {
    use crate::call::{Opener, Sealer, Stream};
    let mut server = Server::default();
    let (mut ana, mut ben, mut eva) = (device(1), device(2), device(3));
    assert!(matches!(ana.call_key(CH), Err(Error::NoGroup)));
    server.enter(&mut ana);
    server.enter(&mut ben);
    server.enter(&mut eva);
    server.sync(&mut ana);
    server.sync(&mut ben);

    let key = ana.call_key(CH).unwrap();
    assert_eq!(ben.call_key(CH).unwrap(), key);
    assert_eq!(eva.call_key(CH).unwrap(), key);
    let mut sealer = Sealer::new(CH, UserId(1), key.clone());
    let mut opener = Opener::new(CH);
    opener.add_key(ben.call_key(CH).unwrap());
    let frame = sealer.seal(Stream::Audio, b"hola").unwrap();
    assert_eq!(
        opener.open(UserId(1), Stream::Audio, &frame).unwrap(),
        b"hola"
    );

    // Eva's device is removed: the next epoch's key is one she cannot derive.
    let p = ana
        .remove(CH, |leaf| leaf.user != UserId(3))
        .unwrap()
        .unwrap();
    let seq = server.commit(UserId(1), &p).unwrap();
    ana.confirm(CH, Some(seq)).unwrap();
    server.sync(&mut ben);
    server.sync(&mut eva);
    let next = ana.call_key(CH).unwrap();
    assert_ne!(next, key);
    assert_eq!(ben.call_key(CH).unwrap(), next);
    assert!(eva.call_key(CH).is_err());
}
