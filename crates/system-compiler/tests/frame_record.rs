//! Frame records (`matter-format.md` section 11, `client-compute.md`
//! sections 4 and 10.2): every declared frame's record agrees with the
//! registry the module emits at two origins, an undeclared frame gives the
//! empty record, every record validates, and the bytes do not depend on the
//! request's origin.

mod common;

use common::system;
use gx_core::frame_record::{self, FrameRecord, RECORD_LEN};
use gx_core::key::ChunkKey;
use gx_core::module_io::{encode_request, encode_tiles, ModuleRequest};
use gx_core::registry;
use gx_module_guest::run_compile;
use system_compiler::conformance::{ORIGIN_A_UTC_MS, ORIGIN_B_UTC_MS};
use system_compiler::{compile, CompileOptions, SystemModule};

/// The module's answer to `key` at `origin_utc_ms`, through its entry point.
fn run(key: &str, origin_utc_ms: i64) -> Vec<u8> {
    let request = encode_request(&ModuleRequest {
        origin_utc_ms,
        span_seconds: 0,
        span_index: 0,
        key: key.parse().unwrap(),
    });
    run_compile::<SystemModule>(&request, &encode_tiles(&[]).unwrap()).unwrap()
}

#[test]
fn every_declared_frame_agrees_with_the_registry_at_two_origins() {
    for origin in [ORIGIN_A_UTC_MS, ORIGIN_B_UTC_MS] {
        let reg = registry::decode(&run("registry", origin)).unwrap();
        assert_eq!(reg.frames().len(), 11);
        for f in reg.frames() {
            let key = format!("frame-{}", f.frame_id);
            let bytes = run(&key, 0);
            frame_record::validate(f.frame_id, &bytes).unwrap();
            let record = frame_record::decode(&bytes).unwrap();
            assert!(!record.empty, "{key}");
            assert_eq!(record.frame_id, f.frame_id);
            assert_eq!(frame_record::agrees(f, &record), Ok(()), "{key}");
        }
    }
}

#[test]
fn an_undeclared_frame_gives_the_empty_record() {
    for id in [11, 42, u64::MAX] {
        let bytes = run(&format!("frame-{id}"), 0);
        assert_eq!(bytes.len(), RECORD_LEN);
        frame_record::validate(id, &bytes).unwrap();
        assert_eq!(
            frame_record::decode(&bytes).unwrap(),
            FrameRecord::empty(id)
        );
        assert_eq!(bytes, frame_record::encode(&FrameRecord::empty(id)));
    }
}

#[test]
fn records_do_not_depend_on_the_origin() {
    for id in [0, 1, 4, 10, 42] {
        let key = format!("frame-{id}");
        let at_zero = run(&key, 0);
        assert_eq!(run(&key, ORIGIN_A_UTC_MS), at_zero, "{key}");
        assert_eq!(run(&key, ORIGIN_B_UTC_MS), at_zero, "{key}");
    }
}

#[test]
fn the_module_is_the_library() {
    let s = system();
    for id in [0, 10, 42] {
        let direct = compile(&s, &ChunkKey::Frame(id), 0, &CompileOptions::default()).unwrap();
        assert_eq!(direct, frame_record::encode(&s.frame_record(id)));
        assert_eq!(run(&format!("frame-{id}"), 0), direct);
    }
}

#[test]
fn the_root_record_has_no_parent_and_bodies_keep_their_moments() {
    let s = system();
    let root = s.frame_record(0);
    assert_eq!(root.parent_frame_id, registry::ROOT_PARENT);
    let body = s.frame_record(10);
    assert_eq!(body.parent_frame_id, 4);
    assert!(body.principal_moments.z > 0.0);
    assert!(body.j2 > 0.0);
}
