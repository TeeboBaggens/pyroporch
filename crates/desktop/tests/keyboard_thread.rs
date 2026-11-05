//! A key runs on a thread of its own, and on a Mac the system ends the program if the keyboard layout
//! is read anywhere but the main thread. That only happens in the real app, so this checks what
//! prevents it: keyboard work asked for on another thread gets done on the main one.

#[cfg(target_os = "macos")]
fn main() {
    use std::time::Duration;

    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        eprintln!("the main thread never took the work on");
        std::process::exit(1);
    });
    std::thread::spawn(|| {
        let on_main = pyroporch_desktop::on_keyboard_thread(|| unsafe { pthread_main_np() } == 1);
        println!("keyboard work asked for on another thread ran on the main one: {on_main}");
        std::process::exit(i32::from(!on_main));
    });
    pyroporch_desktop::serve_main_thread();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    assert_eq!(pyroporch_desktop::on_keyboard_thread(|| 2 + 2), 4);
}
