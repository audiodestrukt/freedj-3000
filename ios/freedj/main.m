// Thin iOS app entry. The real app is the Rust staticlib (opendeck-app); this
// just calls its exported entry, which starts winit's event loop (winit drives
// UIApplicationMain internally and never returns).
//
// Exported from crates/app/src/lib.rs as `freedj_ios_main`.
#import <UIKit/UIKit.h>

extern void freedj_ios_main(void);

// Called from Rust (see set_idle_timer_disabled in lib.rs) to keep the screen
// awake during a set — a locked screen means no touch control, so every DJ app
// disables auto-lock while active.  UIKit must be touched on the main thread;
// winit's event-loop callbacks (where this is called) already run there, but
// hop to the main queue defensively in case that ever changes.
void freedj_set_idle_timer_disabled(bool disabled) {
    if ([NSThread isMainThread]) {
        [UIApplication sharedApplication].idleTimerDisabled = disabled;
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            [UIApplication sharedApplication].idleTimerDisabled = disabled;
        });
    }
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        freedj_ios_main();
    }
    return 0;
}
