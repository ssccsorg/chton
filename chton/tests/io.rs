// IO flow surface tests: the flat key-space FileIo contract.

use chton::io::{FileIo, FsIo};
use futures_executor::block_on;

#[test]
fn fs_io_round_trip() {
    let dir = std::env::temp_dir().join(format!("chton-io-{}", std::process::id()));
    let io = FsIo::new(&dir).unwrap();

    block_on(async {
        io.write("facts/f_a.fact", b"alpha").await.unwrap();
        assert_eq!(
            io.read("facts/f_a.fact").await.unwrap().as_deref(),
            Some(&b"alpha"[..])
        );
        let listed = io.list("facts/").await.unwrap();
        assert_eq!(listed, vec!["facts/f_a.fact".to_string()]);

        io.delete("facts/f_a.fact").await.unwrap();
        assert!(io.read("facts/f_a.fact").await.unwrap().is_none());
    });

    std::fs::remove_dir_all(&dir).unwrap();
}
