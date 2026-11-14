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

// Device idiom, for the Link player-number default: an iPad comes up as
// player 3 and an iPhone as 4, so two OpenDecks on one network do not both
// claim 3 (same-numbered players ignore each other's packets).
bool freedj_is_phone(void) {
    return [UIDevice currentDevice].userInterfaceIdiom == UIUserInterfaceIdiomPhone;
}

// ── USB drives (rekordbox sticks) ────────────────────────────────────────────
//
// iOS shows a plugged-in drive in the Files app but gives no app access to it
// until the user picks it in the folder picker.  The pick hands back a
// security-scoped URL; we hold access to it for the life of the process and
// keep a bookmark (Application Support/opendeck/usb-bookmarks.plist) so the
// same drive opens again without asking, whenever it is plugged in.
//
// Rust (crates/app/src/media.rs) calls freedj_usb_pick from the browser's
// "USB: ADD DRIVE…" row and freedj_usb_paths from its once-a-second media
// scan.  Resolving a bookmark for an absent drive can block, so that runs on a
// background queue and freedj_usb_paths only reads the cached answer.

#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

@interface FreedjUsbPicker : NSObject <UIDocumentPickerDelegate>
@end

static FreedjUsbPicker *gUsbPicker;
static NSMutableDictionary<NSString *, NSURL *> *gUsbActive;   // path → URL we hold access to
static NSString *gUsbPaths = @"";
static dispatch_queue_t gUsbQueue;
static CFAbsoluteTime gUsbLastRefresh;
static const NSUInteger kUsbMaxBookmarks = 8;

static void freedj_usb_init(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        gUsbActive = [NSMutableDictionary dictionary];
        gUsbQueue = dispatch_queue_create("opendeck.usb", DISPATCH_QUEUE_SERIAL);
    });
}

static NSURL *freedj_usb_store(void) {
    NSURL *dir = [[[NSFileManager defaultManager] URLsForDirectory:NSApplicationSupportDirectory
                                                         inDomains:NSUserDomainMask].firstObject
                  URLByAppendingPathComponent:@"opendeck" isDirectory:YES];
    [[NSFileManager defaultManager] createDirectoryAtURL:dir withIntermediateDirectories:YES attributes:nil error:nil];
    return [dir URLByAppendingPathComponent:@"usb-bookmarks.plist"];
}

static NSMutableArray<NSData *> *freedj_usb_load(void) {
    NSArray *a = [NSArray arrayWithContentsOfURL:freedj_usb_store()];
    NSMutableArray<NSData *> *out = [NSMutableArray array];
    for (id d in a) if ([d isKindOfClass:[NSData class]]) [out addObject:d];
    return out;
}

static void freedj_usb_save(NSArray<NSData *> *bookmarks) {
    [bookmarks writeToURL:freedj_usb_store() atomically:YES];
}

static void freedj_usb_publish(void) {
    @synchronized (gUsbActive) {
        gUsbPaths = [[gUsbActive.allKeys sortedArrayUsingSelector:@selector(compare:)] componentsJoinedByString:@"\n"];
    }
}

// Background queue: drop drives that went away, open bookmarked ones that
// came back, refresh stale bookmarks.
static void freedj_usb_refresh(void) {
    NSFileManager *fm = [NSFileManager defaultManager];
    @synchronized (gUsbActive) {
        for (NSString *p in gUsbActive.allKeys) {
            if (![fm fileExistsAtPath:p]) {
                [gUsbActive[p] stopAccessingSecurityScopedResource];
                [gUsbActive removeObjectForKey:p];
            }
        }
    }
    NSMutableArray<NSData *> *bookmarks = freedj_usb_load();
    BOOL changed = NO;
    for (NSUInteger i = 0; i < bookmarks.count; i++) {
        BOOL stale = NO;
        NSURL *url = [NSURL URLByResolvingBookmarkData:bookmarks[i] options:0 relativeToURL:nil
                                   bookmarkDataIsStale:&stale error:nil];
        if (!url) continue;   // drive not plugged in
        NSString *p = url.path;
        @synchronized (gUsbActive) { if (gUsbActive[p]) continue; }
        if (![url startAccessingSecurityScopedResource]) continue;
        if (![fm fileExistsAtPath:p]) { [url stopAccessingSecurityScopedResource]; continue; }
        @synchronized (gUsbActive) { gUsbActive[p] = url; }
        if (stale) {
            NSData *fresh = [url bookmarkDataWithOptions:0 includingResourceValuesForKeys:nil relativeToURL:nil error:nil];
            if (fresh) { bookmarks[i] = fresh; changed = YES; }
        }
    }
    if (changed) freedj_usb_save(bookmarks);
    freedj_usb_publish();
}

@implementation FreedjUsbPicker
- (void)documentPicker:(UIDocumentPickerViewController *)controller didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSURL *url = urls.firstObject;
    if (!url || ![url startAccessingSecurityScopedResource]) return;
    NSData *bm = [url bookmarkDataWithOptions:0 includingResourceValuesForKeys:nil relativeToURL:nil error:nil];
    NSString *path = url.path;
    @synchronized (gUsbActive) { gUsbActive[path] = url; }
    freedj_usb_publish();
    if (!bm) return;
    dispatch_async(gUsbQueue, ^{
        // One bookmark per drive: replace any that resolves to the same path.
        NSMutableArray<NSData *> *bookmarks = freedj_usb_load();
        NSMutableArray<NSData *> *keep = [NSMutableArray arrayWithObject:bm];
        for (NSData *d in bookmarks) {
            BOOL stale = NO;
            NSURL *u = [NSURL URLByResolvingBookmarkData:d options:NSURLBookmarkResolutionWithoutUI
                                           relativeToURL:nil bookmarkDataIsStale:&stale error:nil];
            if (u && [u.path isEqualToString:path]) continue;
            if (keep.count < kUsbMaxBookmarks) [keep addObject:d];
        }
        freedj_usb_save(keep);
    });
}
- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller {}
@end

static UIViewController *freedj_top_controller(void) {
    UIWindow *key = nil;
    for (UIScene *scene in [UIApplication sharedApplication].connectedScenes) {
        if (![scene isKindOfClass:[UIWindowScene class]]) continue;
        for (UIWindow *w in ((UIWindowScene *)scene).windows) {
            if (w.isKeyWindow) { key = w; break; }
            if (!key) key = w;
        }
    }
    UIViewController *vc = key.rootViewController;
    while (vc.presentedViewController) vc = vc.presentedViewController;
    return vc;
}

void freedj_usb_pick(void) {
    freedj_usb_init();
    dispatch_async(dispatch_get_main_queue(), ^{
        UIViewController *top = freedj_top_controller();
        if (!top) return;
        if (!gUsbPicker) gUsbPicker = [FreedjUsbPicker new];
        UIDocumentPickerViewController *picker =
            [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:@[UTTypeFolder]];
        picker.delegate = gUsbPicker;
        picker.allowsMultipleSelection = NO;
        picker.directoryURL = nil;
        [top presentViewController:picker animated:YES completion:nil];
    });
}

size_t freedj_usb_paths(char *buf, size_t cap) {
    freedj_usb_init();
    CFAbsoluteTime now = CFAbsoluteTimeGetCurrent();
    if (now - gUsbLastRefresh > 3.0) {
        gUsbLastRefresh = now;
        dispatch_async(gUsbQueue, ^{ freedj_usb_refresh(); });
    }
    NSString *paths;
    @synchronized (gUsbActive) { paths = gUsbPaths; }
    const char *utf8 = paths.UTF8String;
    size_t n = strlen(utf8);
    if (buf && cap) memcpy(buf, utf8, n < cap ? n : cap);
    return n;
}

// ── Stem separation (HTDemucs through Core ML) ───────────────────────────────
//
// Rust (crates/app/src/separate.rs) splits a track into stems in chunks and
// calls freedj_sep_run for each.  The model is whatever `*demucs*.mlpackage`
// (or compiled `.mlmodelc`) the user put in the OpenDeck folder (Documents,
// or one folder down).  A package is compiled once into Application Support
// and reused while the package is unchanged.  Input "mix" [1, 2, n] float32,
// output "sources" [1, 8, n]: drums, bass, other, vocals, L and R each.

#import <CoreML/CoreML.h>
#import <mach/mach.h>
#import <os/proc.h>
#import <sys/utsname.h>

static void freedj_err(char *err, size_t len, NSString *msg) {
    if (!err || !len) return;
    const char *u = msg.UTF8String ?: "error";
    strlcpy(err, u, len);
}

static NSURL *freedj_sep_find(void) {
    NSFileManager *fm = [NSFileManager defaultManager];
    NSURL *docs = [fm URLsForDirectory:NSDocumentDirectory inDomains:NSUserDomainMask].firstObject;
    NSMutableArray<NSURL *> *dirs = [NSMutableArray arrayWithObject:docs];
    for (NSURL *u in [fm contentsOfDirectoryAtURL:docs includingPropertiesForKeys:@[NSURLIsDirectoryKey] options:NSDirectoryEnumerationSkipsHiddenFiles error:nil]) {
        NSNumber *isDir = nil;
        [u getResourceValue:&isDir forKey:NSURLIsDirectoryKey error:nil];
        if (isDir.boolValue && ![u.pathExtension isEqualToString:@"mlpackage"] && ![u.pathExtension isEqualToString:@"mlmodelc"]) [dirs addObject:u];
    }
    for (NSURL *d in dirs) {
        for (NSURL *u in [fm contentsOfDirectoryAtURL:d includingPropertiesForKeys:nil options:NSDirectoryEnumerationSkipsHiddenFiles error:nil]) {
            NSString *ext = u.pathExtension;
            if (([ext isEqualToString:@"mlpackage"] || [ext isEqualToString:@"mlmodelc"])
                && [u.lastPathComponent.lowercaseString containsString:@"demucs"]) return u;
        }
    }
    return nil;
}

bool freedj_sep_model_present(void) { return freedj_sep_find() != nil; }

static NSDate *freedj_mtime(NSURL *u) {
    NSDate *d = nil;
    [u getResourceValue:&d forKey:NSURLContentModificationDateKey error:nil];
    return d ?: [NSDate distantPast];
}

void *freedj_sep_open(char *err, size_t errlen) {
    @autoreleasepool {
        if (@available(iOS 18.0, *)) {
            NSURL *src = freedj_sep_find();
            if (!src) { freedj_err(err, errlen, @"no *demucs*.mlpackage in the OpenDeck folder"); return NULL; }
            NSError *e = nil;
            NSURL *compiled = src;
            if ([src.pathExtension isEqualToString:@"mlpackage"]) {
                NSFileManager *fm = [NSFileManager defaultManager];
                NSURL *dir = [[[fm URLsForDirectory:NSApplicationSupportDirectory inDomains:NSUserDomainMask].firstObject
                               URLByAppendingPathComponent:@"opendeck" isDirectory:YES] URLByAppendingPathComponent:@"models" isDirectory:YES];
                [fm createDirectoryAtURL:dir withIntermediateDirectories:YES attributes:nil error:nil];
                compiled = [dir URLByAppendingPathComponent:[src.URLByDeletingPathExtension.lastPathComponent stringByAppendingString:@".mlmodelc"]];
                NSURL *weights = [src URLByAppendingPathComponent:@"Data/com.apple.CoreML/weights/weight.bin"];
                BOOL fresh = [fm fileExistsAtPath:compiled.path]
                    && [freedj_mtime(compiled) compare:freedj_mtime(weights)] == NSOrderedDescending;
                if (!fresh) {
                    NSURL *tmp = [MLModel compileModelAtURL:src error:&e];
                    if (!tmp) { freedj_err(err, errlen, [NSString stringWithFormat:@"compile: %@", e.localizedDescription]); return NULL; }
                    [fm removeItemAtURL:compiled error:nil];
                    if (![fm moveItemAtURL:tmp toURL:compiled error:&e]) {
                        freedj_err(err, errlen, [NSString stringWithFormat:@"keep compiled model: %@", e.localizedDescription]); return NULL;
                    }
                }
            }
            MLModelConfiguration *cfg = [MLModelConfiguration new];
            cfg.computeUnits = MLComputeUnitsAll;
            MLModel *m = [MLModel modelWithContentsOfURL:compiled configuration:cfg error:&e];
            if (!m) { freedj_err(err, errlen, [NSString stringWithFormat:@"load: %@", e.localizedDescription]); return NULL; }
            return (__bridge_retained void *)m;
        }
        freedj_err(err, errlen, @"stem splitting needs iOS 18");
        return NULL;
    }
}

size_t freedj_sep_segment(void *h) {
    MLModel *m = (__bridge MLModel *)h;
    MLFeatureDescription *d = m.modelDescription.inputDescriptionsByName[@"mix"];
    NSArray<NSNumber *> *shape = d.multiArrayConstraint.shape;
    return shape.count == 3 ? shape[2].unsignedIntegerValue : 0;
}

bool freedj_sep_run(void *h, const float *mix, float *dst, size_t n, char *err, size_t errlen) {
    if (@available(iOS 18.0, *)) {
        @autoreleasepool {
            MLModel *m = (__bridge MLModel *)h;
            NSError *e = nil;
            MLMultiArray *mixArr = [[MLMultiArray alloc] initWithDataPointer:(void *)mix
                                                                       shape:@[@1, @2, @(n)]
                                                                    dataType:MLMultiArrayDataTypeFloat32
                                                                     strides:@[@(2 * n), @(n), @1]
                                                                 deallocator:nil
                                                                       error:&e];
            if (!mixArr) { freedj_err(err, errlen, [NSString stringWithFormat:@"input: %@", e.localizedDescription]); return false; }
            MLDictionaryFeatureProvider *fp = [[MLDictionaryFeatureProvider alloc] initWithDictionary:@{@"mix": mixArr} error:&e];
            if (!fp) { freedj_err(err, errlen, [NSString stringWithFormat:@"input: %@", e.localizedDescription]); return false; }
            id<MLFeatureProvider> res = [m predictionFromFeatures:fp error:&e];
            if (!res) { freedj_err(err, errlen, [NSString stringWithFormat:@"predict: %@", e.localizedDescription]); return false; }
            MLMultiArray *o = [res featureValueForName:@"sources"].multiArrayValue;
            if (!o || o.shape.count != 3 || o.shape[1].unsignedIntegerValue != 8 || o.shape[2].unsignedIntegerValue != n) {
                freedj_err(err, errlen, [NSString stringWithFormat:@"output shape %@", o.shape]); return false;
            }
            NSInteger s1 = o.strides[1].integerValue, s2 = o.strides[2].integerValue;
            MLMultiArrayDataType t = o.dataType;
            __block bool ok = true;
            [o getBytesWithHandler:^(const void *bytes, NSInteger size) {
                (void)size;
                for (size_t c = 0; c < 8; c++) {
                    for (size_t i = 0; i < n; i++) {
                        NSInteger k = (NSInteger)c * s1 + (NSInteger)i * s2;
                        float v;
                        if (t == MLMultiArrayDataTypeFloat32) v = ((const float *)bytes)[k];
                        else if (t == MLMultiArrayDataTypeFloat16) v = (float)((const __fp16 *)bytes)[k];
                        else if (t == MLMultiArrayDataTypeDouble) v = (float)((const double *)bytes)[k];
                        else { ok = false; return; }
                        dst[c * n + i] = v;
                    }
                }
            }];
            if (!ok) freedj_err(err, errlen, @"output type not float");
            return ok;
        }
    }
    freedj_err(err, errlen, @"stem splitting needs iOS 18");
    return false;
}

// Process memory in bytes: what iOS counts against us, and what is left
// before it would kill the app.
uint64_t freedj_mem_footprint(void) {
    task_vm_info_data_t info;
    mach_msg_type_number_t count = TASK_VM_INFO_COUNT;
    if (task_info(mach_task_self(), TASK_VM_INFO, (task_info_t)&info, &count) != KERN_SUCCESS) return 0;
    return info.phys_footprint;
}

uint64_t freedj_mem_available(void) { return os_proc_available_memory(); }

void freedj_device_model(char *buf, size_t len) {
    struct utsname u;
    uname(&u);
    strlcpy(buf, u.machine, len);
}

int main(int argc, char *argv[]) {
    @autoreleasepool {
        freedj_ios_main();
    }
    return 0;
}
