// Thin iOS app entry. The real app is the Rust staticlib (opendeck-app); this
// just calls its exported entry, which starts winit's event loop (winit drives
// UIApplicationMain internally and never returns).
//
// Exported from crates/app/src/lib.rs as `freedj_ios_main`.
#import <UIKit/UIKit.h>

extern void freedj_ios_main(void);

int main(int argc, char *argv[]) {
    @autoreleasepool {
        freedj_ios_main();
    }
    return 0;
}
