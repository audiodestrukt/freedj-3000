//! Desktop binary entry — all logic lives in the library (`opendeck_app`) so it
//! can also build as a staticlib for iOS.  See ios/README.md.

fn main() -> anyhow::Result<()> {
    opendeck_app::desktop_main()
}
