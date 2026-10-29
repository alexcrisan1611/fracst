//! Client and server over a real WebSocket: remote frames must equal local
//! ones bit for bit, tokens must be checked, and cancelling must not leave
//! stale data behind for the next frame.

use fracst_core::{PixelEngine, Progress, Renderer, Settings, View};
use fracst_cuda::check::{cases, differing_pixels};
use fracst_cuda::host::HostEngine;
use fracst_remote::Client;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

fn start(engine: Option<Arc<dyn PixelEngine>>, token: Option<&str>) -> String {
    start_with_idle(engine, token, None)
}

fn start_with_idle(engine: Option<Arc<dyn PixelEngine>>, token: Option<&str>, idle: Option<Duration>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let config = fracst_server::Config {
        engine,
        engine_name: "test".into(),
        token: token.map(String::from),
        max_pixels: 1 << 24,
        idle_timeout: idle,
    };
    std::thread::spawn(move || fracst_server::run(listener, config));
    url
}

fn local(view: &View, s: &Settings) -> fracst_core::Frame {
    Renderer::new().render(view, s, &Progress::default()).unwrap()
}

#[test]
fn remote_frames_match_local() {
    // The server iterates pixels with the kernel's code (host build), as it would on a GPU.
    let url = start(Some(Arc::new(HostEngine)), None);
    let mut client = Client::connect(&url, None).unwrap();
    assert_eq!(client.engine, "test");
    // The server keeps one renderer per connection, reusing reference orbits
    // between requests; so does this one.
    let mut renderer = Renderer::new();
    for case in cases() {
        let remote = client.render(&case.view, &case.settings, &Progress::default()).unwrap().unwrap();
        let expected = renderer.render(&case.view, &case.settings, &Progress::default()).unwrap();
        assert_eq!(differing_pixels(&expected, &remote), Some(0), "{}", case.name);
        assert_eq!(remote.stats.steps, expected.stats.steps, "{}", case.name);
        assert_eq!(remote.stats.suggested_maxiter, expected.stats.suggested_maxiter, "{}", case.name);
    }
}

#[test]
fn token_is_required() {
    let url = start(None, Some("secret"));
    let err = Client::connect(&url, None).err().expect("connecting without the token must fail");
    assert!(err.contains("401"), "{err}");
    assert!(Client::connect(&url, Some("wrong")).is_err());
    let mut client = Client::connect(&url, Some("secret")).unwrap();
    let view = View { maxiter: 500, ..View::default() };
    let s = Settings { width: 64, height: 48, ..Settings::default() };
    assert!(client.render(&view, &s, &Progress::default()).unwrap().is_some());
}

#[test]
fn cancelled_frames_do_not_leak_into_the_next() {
    let url = start(None, None);
    let mut client = Client::connect(&url, None).unwrap();
    let heavy =
        View::parse("-0.743643887037158704752191506114774", "0.131825904205311970493132056385139", "1e7", 200_000)
            .unwrap();
    let big = Settings { width: 1600, height: 1200, interior_detection: false, ..Settings::default() };
    for wait in [0, 30, 300] {
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(wait));
            p.cancel();
        });
        let _ = client.render(&heavy, &big, &progress).unwrap();
        // Whatever was in flight, the next frame must be exactly the next
        // view's (with a fresh reference: the cancelled render may have left
        // one behind that a later render would reuse).
        let view = View { maxiter: 300, ..View::default() };
        let s = Settings { width: 96, height: 64, reuse_reference: false, ..Settings::default() };
        let frame = client.render(&view, &s, &Progress::default()).unwrap().unwrap();
        assert_eq!(differing_pixels(&local(&view, &s), &frame), Some(0), "after cancelling at {wait} ms");
    }
}

#[test]
fn bad_requests_get_errors() {
    let url = start(None, None);
    let mut client = Client::connect(&url, None).unwrap();
    let view = View::default();
    let huge = Settings { width: 100_000, height: 100_000, ..Settings::default() };
    let err = client.render(&view, &huge, &Progress::default()).unwrap_err();
    assert!(err.contains("outside"), "{err}");
    // The connection stays usable.
    let s = Settings { width: 32, height: 32, ..Settings::default() };
    assert!(client.render(&view, &s, &Progress::default()).unwrap().is_some());
}

#[test]
fn idle_connections_are_closed() {
    let url = start_with_idle(None, None, Some(Duration::from_millis(600)));
    let mut client = Client::connect(&url, None).unwrap();
    let view = View { maxiter: 200, ..View::default() };
    let s = Settings { width: 32, height: 32, ..Settings::default() };
    assert!(client.render(&view, &s, &Progress::default()).unwrap().is_some());
    std::thread::sleep(Duration::from_millis(1200));
    let err = client.render(&view, &s, &Progress::default()).unwrap_err();
    assert!(err.contains("idle"), "{err}");
}

#[test]
fn queued_requests_come_back_in_order() {
    let url = start(None, None);
    let mut client = Client::connect(&url, None).unwrap();
    assert!(client.can_queue());
    let s = Settings { width: 96, height: 64, ..Settings::default() };
    let views: Vec<View> = (0..4)
        .map(|i| View { zoom: fracst_core::FloatExp::from(1.0 + i as f64), maxiter: 500, ..View::default() })
        .collect();
    // All requests go out before any frame is read: none may cancel another.
    let ids: Vec<u64> = views.iter().map(|v| client.submit(v, &s, true).unwrap()).collect();
    for (id, view) in ids.into_iter().zip(&views) {
        let frame = client.wait(id, &Progress::default()).unwrap().unwrap();
        assert_eq!(frame.n, local(view, &s).n);
    }
}
