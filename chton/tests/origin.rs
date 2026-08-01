use chton::origin::{
    AddressMode, Binding, Direction, FileOrigin, MemoryOrigin, Origin, OriginError, Persistence,
};

#[test]
fn memory_round_trip() {
    let mut origin = MemoryOrigin::new();
    origin.write(0, b"hello").unwrap();
    let mut buf = [0u8; 5];
    let n = origin.read(0, &mut buf).unwrap();
    assert_eq!(n, 5);
    assert_eq!(&buf, b"hello");
    assert_eq!(origin.len(), 5);
}

#[test]
fn memory_write_extends_region() {
    let mut origin = MemoryOrigin::new();
    origin.write(10, b"x").unwrap();
    assert_eq!(origin.len(), 11);
    let mut buf = [0u8; 11];
    let n = origin.read(0, &mut buf).unwrap();
    assert_eq!(n, 11);
    assert_eq!(&buf[..10], &[0u8; 10]);
    assert_eq!(buf[10], b'x');
}

#[test]
fn memory_read_out_of_bounds_is_error() {
    let origin = MemoryOrigin::new();
    let mut buf = [0u8; 1];
    assert!(matches!(
        origin.read(1, &mut buf),
        Err(OriginError::OutOfBounds { .. })
    ));
}

#[test]
fn memory_capabilities() {
    let origin = MemoryOrigin::new();
    let caps = origin.capabilities();
    assert_eq!(caps.address_mode, AddressMode::Byte);
    assert_eq!(caps.direction, Direction::Duplex);
    assert_eq!(caps.persistence, Persistence::Volatile);
    assert_eq!(caps.binding, Binding::Copied);
}

#[test]
fn file_round_trip() {
    let path = std::env::temp_dir().join(format!("chton-origin-{}.bin", std::process::id()));
    let mut origin = FileOrigin::open(&path).unwrap();
    origin.write(4, b"abcd").unwrap();
    assert_eq!(origin.len(), 8);
    let mut buf = [0u8; 4];
    let n = origin.read(4, &mut buf).unwrap();
    assert_eq!(n, 4);
    assert_eq!(&buf, b"abcd");
    origin.flush().unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn file_persists_across_reopen() {
    let path = std::env::temp_dir().join(format!("chton-persist-{}.bin", std::process::id()));
    {
        let mut origin = FileOrigin::open(&path).unwrap();
        origin.write(0, b"persist").unwrap();
        origin.flush().unwrap();
    }
    {
        let origin = FileOrigin::open(&path).unwrap();
        assert_eq!(origin.len(), 7);
        let mut buf = [0u8; 7];
        let n = origin.read(0, &mut buf).unwrap();
        assert_eq!(n, 7);
        assert_eq!(&buf, b"persist");
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn file_capabilities() {
    let path = std::env::temp_dir().join(format!("chton-caps-{}.bin", std::process::id()));
    let origin = FileOrigin::open(&path).unwrap();
    let caps = origin.capabilities();
    assert_eq!(caps.address_mode, AddressMode::Byte);
    assert_eq!(caps.direction, Direction::Duplex);
    assert_eq!(caps.persistence, Persistence::Durable);
    assert_eq!(caps.binding, Binding::Copied);
    std::fs::remove_file(&path).unwrap();
}
