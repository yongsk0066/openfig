#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <InputMethodKit/InputMethodKit.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <stdatomic.h>
#include <unistd.h>

#ifdef FIG_IME_FIXTURE
#ifndef FIG_IME_TEST_BUNDLE_ID
#define FIG_IME_TEST_BUNDLE_ID @"local.openfig.fig.inputmethod.test"
#endif
static NSString *const BundleID = FIG_IME_TEST_BUNDLE_ID;
#ifndef FIG_IME_TEST_CONNECTION_NAME
#define FIG_IME_TEST_CONNECTION_NAME @"local.openfig.fig.inputmethod.test.connection"
#endif
static NSString *const ConnectionName = FIG_IME_TEST_CONNECTION_NAME;
static NSString *const SocketName = @"figrun/ime-test.sock";
#else
static NSString *BundleID;
static NSString *ConnectionName;
static NSString *const SocketName = @"figrun/ime.sock";
#endif
static atomic_uint_fast64_t ProbeStarted;

@class FigInputController;
static __weak FigInputController *ActiveController; // Accessed only on the AppKit thread.

@interface Transport : NSObject
@property NSLock *lock;
@property dispatch_queue_t writer;
@property int fd;
@property uint64_t generation;
@property(atomic) BOOL busy;
- (void)start;
- (void)send:(NSDictionary *)message generation:(uint64_t)generation;
- (void)focusChanged;
@end
static Transport *Bridge;

@interface FigInputController : IMKInputController
@end

@interface PaletteLifecycle : NSObject <NSApplicationDelegate>
@property IMKServer *server;
@property(copy) void (^shutdownReply)(BOOL);
@property unsigned shutdownAttempts;
- (void)requestTermination:(void (^)(BOOL))reply;
@end

@implementation PaletteLifecycle
- (void)finishTermination:(BOOL)detached {
    NSLog(@"Palette termination detached=%d attempts=%u", detached, self.shutdownAttempts);
    void (^reply)(BOOL) = self.shutdownReply;
    self.shutdownReply = nil;
    if (!detached) [Bridge send:@{@"v": @1, @"type": @"ready"} generation:Bridge.generation];
    reply(detached);
}
- (void)attemptTermination {
    BOOL detached = NO;
    self.shutdownAttempts++;
    atomic_store(&ProbeStarted, clock_gettime_nsec_np(CLOCK_UPTIME_RAW));
    @try {
        detached = [self.server paletteWillTerminate];
    } @catch (NSException *exception) {
        atomic_store(&ProbeStarted, 0);
        NSLog(@"Palette termination failed: %@", exception.name);
        [self finishTermination:NO]; return;
    }
    atomic_store(&ProbeStarted, 0);
    if (detached || self.shutdownAttempts >= 20) {
        [self finishTermination:detached]; return;
    }
    // A client acknowledgement needs the main run loop. NO never authorizes exit.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
        [self attemptTermination];
    });
}
- (void)requestTermination:(void (^)(BOOL))reply {
    if (self.shutdownReply) return;
    self.shutdownReply = reply;
    self.shutdownAttempts = 0;
    [Bridge send:@{@"v": @1, @"type": @"retiring"} generation:Bridge.generation];
    // The delegate must return NSTerminateLater before AppKit receives its reply.
    dispatch_async(dispatch_get_main_queue(), ^{ [self attemptTermination]; });
}
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender {
    [self requestTermination:^(BOOL detached) { [sender replyToApplicationShouldTerminate:detached]; }];
    return NSTerminateLater;
}
@end

static BOOL IsSupportedTarget(NSString *bundle) {
#ifdef FIG_IME_FIXTURE
    return [bundle isEqual:@"local.openfig.fig.imefixture"];
#else
    return [bundle isEqual:@"com.mitchellh.ghostty"] || [bundle isEqual:@"com.mitchellh.ghostty.debug"];
#endif
}

static id AttachmentState(BOOL foreground, BOOL attached) {
    // BOOL boxing produces JSON booleans; boxing a C logical expression produces an integer.
    return foreground ? @(attached) : NSNull.null;
}

// IMK returns Cocoa screen points. Convert once at the process boundary; never multiply by DPI.
static NSArray *QuartzRect(NSRect rect, CGFloat primaryTop) {
    double values[] = {rect.origin.x, primaryTop - NSMaxY(rect), rect.size.width, rect.size.height};
    for (unsigned i = 0; i < 4; i++) if (!isfinite(values[i])) return nil;
    if (values[2] < 0 || values[3] <= 0) return nil;
    return @[@(values[0]), @(values[1]), @(values[2]), @(values[3])];
}

static NSArray *FrameContainingRect(NSArray *rect, pid_t pid, CGWindowID window) {
    NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow, window));
    for (NSDictionary *info in windows) {
        if ([info[(__bridge NSString *)kCGWindowNumber] unsignedIntValue] != window ||
            [info[(__bridge NSString *)kCGWindowOwnerPID] intValue] != pid) continue;
        CGRect frame;
        if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)info[(__bridge NSString *)kCGWindowBounds], &frame)) return nil;
        CGRect caret = CGRectMake([rect[0] doubleValue], [rect[1] doubleValue], [rect[2] doubleValue], [rect[3] doubleValue]);
        if (!CGRectContainsRect(frame, CGRectInset(caret, -0.01, 0))) return nil;
        return @[@(frame.origin.x), @(frame.origin.y), @(frame.size.width), @(frame.size.height)];
    }
    return nil;
}

static void Query(NSDictionary *request, uint64_t generation) {
    NSCAssert(NSThread.isMainThread, @"IMK clients must stay on their owning thread");
    id result = NSNull.null;
    id windowFrame = NSNull.null;
    BOOL suppressed = NO;
    id attachment = NSNull.null;
    atomic_store(&ProbeStarted, clock_gettime_nsec_np(CLOCK_UPTIME_RAW));
    @try {
        FigInputController *controller = ActiveController;
        id<IMKTextInput, NSObject> client = controller.client;
        NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
        pid_t pid = [request[@"pid"] intValue];
        attachment = AttachmentState(IsSupportedTarget(front.bundleIdentifier) && front.processIdentifier == pid,
                                     client && [client.bundleIdentifier isEqual:front.bundleIdentifier]);
        if (getenv("FIG_IME_TRACE")) {
            NSLog(@"Caret client=%@ active=%d foreground=%@ pid=%d requested=%d", client.bundleIdentifier,
                  controller != nil, front.bundleIdentifier, front.processIdentifier, pid);
        }
        if (client && IsSupportedTarget(client.bundleIdentifier) && IsSupportedTarget(front.bundleIdentifier) && front.processIdentifier == pid) {
            NSRange marked = client.markedRange;
            NSRange selection = client.selectedRange;
            suppressed = (marked.location != NSNotFound && marked.length > 0) ||
                (selection.location != NSNotFound && selection.length > 0);
            // Do not compete with the active keyboard IME's composition/candidate window.
            if ((marked.location == NSNotFound || marked.length == 0) &&
                (selection.location == NSNotFound || selection.length == 0)) {
                NSRect rect = NSZeroRect;
                // Index zero means current insertion point, not document offset zero.
                (void)[client attributesForCharacterIndex:0 lineHeightRectangle:&rect];
                NSScreen *primary = NSScreen.screens.firstObject;
                NSArray *values = primary ? QuartzRect(rect, NSMaxY(primary.frame)) : nil;
                front = NSWorkspace.sharedWorkspace.frontmostApplication;
                NSArray *frame = values ? FrameContainingRect(values, pid, [request[@"window"] unsignedIntValue]) : nil;
                if (frame && controller == ActiveController && front.processIdentifier == pid) {
                    result = values;
                    windowFrame = frame;
                }
            }
        }
    } @catch (NSException *exception) {
        NSLog(@"Input method query failed: %@", exception.name);
    }
    atomic_store(&ProbeStarted, 0);
    Bridge.busy = NO;
    [Bridge send:@{@"v": @1, @"type": @"rect", @"id": request[@"id"], @"rect": result, @"frame": windowFrame, @"suppressed": @(suppressed), @"attachment": attachment} generation:generation];
}

@implementation FigInputController
- (instancetype)initWithServer:(IMKServer *)server delegate:(id)delegate client:(id)client {
    self = [super initWithServer:server delegate:delegate client:client];
    if (getenv("FIG_IME_TRACE")) NSLog(@"Created controller for %@", self.client.bundleIdentifier);
    return self;
}
- (void)activateServer:(id)sender {
    (void)sender;
    ActiveController = self;
    if (getenv("FIG_IME_TRACE")) NSLog(@"Activated %@", self.client.bundleIdentifier);
    [Bridge focusChanged];
}
- (void)deactivateServer:(id)sender {
    (void)sender;
    if (ActiveController == self) { ActiveController = nil; [Bridge focusChanged]; }
}
- (void)inputControllerWillClose {
    if (ActiveController == self) { ActiveController = nil; [Bridge focusChanged]; }
    [super inputControllerWillClose];
}
- (NSUInteger)recognizedEvents:(id)sender { (void)sender; return 0; }
- (BOOL)handleEvent:(NSEvent *)event client:(id)sender { (void)event; (void)sender; return NO; }
@end

@implementation Transport
- (instancetype)init {
    if ((self = [super init])) {
        _lock = [NSLock new]; _fd = -1;
        _writer = dispatch_queue_create("local.openfig.fig.inputmethod.writer", DISPATCH_QUEUE_SERIAL);
    }
    return self;
}
- (void)send:(NSDictionary *)message generation:(uint64_t)generation {
    NSData *json = [NSJSONSerialization dataWithJSONObject:message options:0 error:nil];
    if (!json || json.length > 4095) return;
    NSMutableData *frame = [json mutableCopy]; [frame appendBytes:"\n" length:1];
    dispatch_async(self.writer, ^{
        [self.lock lock];
        if (self.fd >= 0 && self.generation == generation) {
            const char *bytes = frame.bytes; size_t left = frame.length;
            while (left) {
                ssize_t sent = send(self.fd, bytes, left, 0);
                if (sent < 0 && errno == EINTR) continue;
                if (sent <= 0) { shutdown(self.fd, SHUT_RDWR); break; }
                bytes += sent; left -= (size_t)sent;
            }
        }
        [self.lock unlock];
    });
}
- (void)focusChanged {
    // Reading the atomic generation never waits for a socket write.
    [self send:@{@"v": @1, @"type": @"focus"} generation:self.generation];
}
- (void)start {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        for (;;) { @autoreleasepool {
            NSString *path = [NSTemporaryDirectory() stringByAppendingPathComponent:SocketName];
            struct sockaddr_un address = { .sun_family = AF_UNIX };
            const char *utf8 = path.fileSystemRepresentation;
            if (strlen(utf8) >= sizeof(address.sun_path)) return;
            strlcpy(address.sun_path, utf8, sizeof(address.sun_path));
            address.sun_len = sizeof(address);
            int fd = socket(AF_UNIX, SOCK_STREAM, 0);
            if (fd < 0) { usleep(500000); continue; }
            int yes = 1; setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, sizeof(yes));
            struct timeval timeout = {.tv_sec = 0, .tv_usec = 200000};
            setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
            uid_t uid; gid_t gid;
            if (connect(fd, (struct sockaddr *)&address, sizeof(address)) != 0 ||
                getpeereid(fd, &uid, &gid) != 0 || uid != getuid()) {
                close(fd); usleep(500000); continue;
            }
            [self.lock lock]; self.fd = fd; uint64_t generation = ++self.generation; [self.lock unlock];
            [self send:@{@"v": @1, @"type": @"ready"} generation:generation];
            NSMutableData *pending = [NSMutableData data];
            BOOL connected = YES;
            while (connected) { @autoreleasepool {
                char bytes[4096]; ssize_t count = recv(fd, bytes, sizeof(bytes), 0);
                if (count < 0 && errno == EINTR) continue;
                if (count <= 0) break;
                [pending appendBytes:bytes length:(NSUInteger)count];
                if (pending.length > 4096) break;
                const char *end = memchr(pending.bytes, '\n', pending.length);
                if (!end) continue;
                NSUInteger length = (NSUInteger)(end - (const char *)pending.bytes) + 1;
                // The desktop admits only one query at a time; reject flooding and framing drift.
                if (length != pending.length || self.busy) break;
                id request = [NSJSONSerialization JSONObjectWithData:pending options:0 error:nil];
                [pending setLength:0];
                if (![request isKindOfClass:NSDictionary.class] || ![request[@"v"] isEqual:@1] ||
                    ![request[@"type"] isEqual:@"query"] || ![request[@"id"] isKindOfClass:NSString.class] ||
                    [request[@"id"] length] != 36 || ![request[@"pid"] isKindOfClass:NSNumber.class] ||
                    ![request[@"window"] isKindOfClass:NSNumber.class] || !IsSupportedTarget(request[@"bundle"])) { connected = NO; continue; }
                self.busy = YES;
                dispatch_async(dispatch_get_main_queue(), ^{ @autoreleasepool { Query(request, generation); } });
            }}
            [self.lock lock]; self.fd = -1; close(fd); [self.lock unlock];
            usleep(250000);
        }}
    });
}
@end

static TISInputSourceRef InputSource(void) {
    NSDictionary *filter = @{(__bridge NSString *)kTISPropertyInputSourceID: BundleID,
        (__bridge NSString *)kTISPropertyBundleID: BundleID,
        (__bridge NSString *)kTISPropertyInputSourceCategory: (__bridge NSString *)kTISCategoryPaletteInputSource};
    NSArray *sources = CFBridgingRelease(TISCreateInputSourceList((__bridge CFDictionaryRef)filter, true));
    return sources.count == 1 ? (TISInputSourceRef)CFRetain((__bridge CFTypeRef)sources.firstObject) : NULL;
}

static BOOL Property(TISInputSourceRef source, CFStringRef key) {
    CFTypeRef value = TISGetInputSourceProperty(source, key);
    return value && CFGetTypeID(value) == CFBooleanGetTypeID() && CFBooleanGetValue(value);
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
#ifndef FIG_IME_FIXTURE
        BundleID = NSBundle.mainBundle.bundleIdentifier;
        ConnectionName = [NSBundle.mainBundle objectForInfoDictionaryKey:@"InputMethodConnectionName"];
        if (!BundleID.length || !ConnectionName.length) {
            fprintf(stderr, "Input method requires its configured app bundle\n"); return 2;
        }
#endif
        if (argc == 2 && strcmp(argv[1], "--stop-running") == 0) {
            NSArray<NSRunningApplication *> *running = [NSRunningApplication runningApplicationsWithBundleIdentifier:BundleID];
            for (NSRunningApplication *app in running) {
                if (app.processIdentifier != getpid()) [app terminate];
            }
            for (unsigned attempt = 0; attempt < 300; attempt++) {
                BOOL stopped = YES;
                for (NSRunningApplication *app in running) {
                    if (app.processIdentifier != getpid() && !app.terminated) {
                        stopped = NO;
                        // Older helpers may cancel the first quit while IMK clients detach.
                        // Retry only the original instances, never a replacement launched by IMK.
                        if (attempt > 0 && attempt % 10 == 0) [app terminate];
                    }
                }
                if (stopped) return 0;
                [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
            }
            fprintf(stderr, "Input method did not stop; existing input-source approval is unchanged\n");
            return 3;
        }
        if (argc == 2 && strcmp(argv[1], "--keyboard-source") == 0) {
            TISInputSourceRef keyboard = TISCopyCurrentKeyboardInputSource();
            if (!keyboard) return 1;
            CFStringRef value = TISGetInputSourceProperty(keyboard, kTISPropertyInputSourceID);
            if (value) puts([(__bridge NSString *)value UTF8String]);
            CFRelease(keyboard); return value ? 0 : 1;
        }
        if (argc == 3 && strcmp(argv[1], "--register") == 0) {
            NSURL *url = [NSURL fileURLWithPath:@(argv[2]) isDirectory:YES];
            if (![[NSBundle bundleWithURL:url].bundleIdentifier isEqual:BundleID]) {
                fprintf(stderr, "Refusing to register a different input method\n"); return 2;
            }
            OSStatus status = TISRegisterInputSource((__bridge CFURLRef)url);
            printf("register: %d\n", (int)status); return status == noErr ? 0 : 1;
        }
        if (argc == 2) {
            TISInputSourceRef source = InputSource();
            if (!source) { fprintf(stderr, "Input method is not registered\n"); return 4; }
            OSStatus status = noErr;
            if (strcmp(argv[1], "--select") == 0) {
                if (!Property(source, kTISPropertyInputSourceIsEnabled)) {
                    fprintf(stderr, "Input method must be enabled before selection\n");
                    CFRelease(source); return 3;
                }
                status = TISSelectInputSource(source);
            } else if (strcmp(argv[1], "--disable") == 0) {
                if (Property(source, kTISPropertyInputSourceIsSelected)) status = TISDeselectInputSource(source);
                if (status == noErr && Property(source, kTISPropertyInputSourceIsEnabled)) status = TISDisableInputSource(source);
                for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:BundleID]) [running terminate];
            } else if (strcmp(argv[1], "--status") != 0 && strcmp(argv[1], "--check-active") != 0 &&
                       strcmp(argv[1], "--check-enabled") != 0) { CFRelease(source); return 2; }
            BOOL enabled = Property(source, kTISPropertyInputSourceIsEnabled);
            BOOL selected = Property(source, kTISPropertyInputSourceIsSelected);
            printf("{\"status\":%d,\"enabled\":%s,\"selected\":%s,\"enable_capable\":%s,\"select_capable\":%s}\n",
                (int)status, enabled ? "true" : "false", selected ? "true" : "false",
                Property(source, kTISPropertyInputSourceIsEnableCapable) ? "true" : "false",
                Property(source, kTISPropertyInputSourceIsSelectCapable) ? "true" : "false");
            CFRelease(source);
            if (strcmp(argv[1], "--check-enabled") == 0) return enabled ? 0 : 3;
            if (strcmp(argv[1], "--check-active") == 0) return enabled && selected ? 0 : 3;
            return status == noErr ? 0 : 1;
        }
        if (argc != 1) { fprintf(stderr, "Unsupported input method arguments\n"); return 2; }
        NSApplication *app = NSApplication.sharedApplication;
        Bridge = [Transport new];
        IMKServer *server = [[IMKServer alloc] initWithName:ConnectionName bundleIdentifier:BundleID];
        if (!server) return 1;
        PaletteLifecycle *lifecycle = [PaletteLifecycle new];
        lifecycle.server = server;
        app.delegate = lifecycle;
        [Bridge start];
        dispatch_source_t watchdog = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0,
            dispatch_get_global_queue(QOS_CLASS_UTILITY, 0));
        dispatch_source_set_timer(watchdog, dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), NSEC_PER_SEC, NSEC_PER_MSEC * 100);
        dispatch_source_set_event_handler(watchdog, ^{
            uint64_t started = atomic_load(&ProbeStarted);
            if (started && clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - started > 2 * NSEC_PER_SEC) _exit(75);
        });
        dispatch_resume(watchdog);
        [app run];
        // Retain the server and watchdog for the entire AppKit run loop.
        [lifecycle description]; dispatch_source_cancel(watchdog);
    }
    return 0;
}
