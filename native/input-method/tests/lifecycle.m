#define FIG_IME_FIXTURE 1
#define main FigInputMethodMain
#include "../main.m"
#undef main

@interface ServerDouble : NSObject
@property NSUInteger readyAfter;
@property BOOL fails;
@property NSUInteger calls;
- (BOOL)paletteWillTerminate;
@end
@implementation ServerDouble
- (BOOL)paletteWillTerminate {
    self.calls++;
    if (self.fails) [NSException raise:@"FixtureFailure" format:@"No client acknowledgement"];
    return self.calls >= self.readyAfter;
}
@end

static BOOL Negotiate(ServerDouble *server) {
    PaletteLifecycle *lifecycle = [PaletteLifecycle new];
    lifecycle.server = (IMKServer *)server;
    __block BOOL done = NO, detached = NO;
    [lifecycle requestTermination:^(BOOL result) { detached = result; done = YES; }];
    NSCAssert(!done, @"The delegate reply must be asynchronous");
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:3];
    while (!done && deadline.timeIntervalSinceNow > 0) {
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.01]];
    }
    NSCAssert(done, @"Termination must have a bounded decision");
    return detached;
}
int main(void) {
    @autoreleasepool {
        ServerDouble *server = [ServerDouble new];
        server.readyAfter = 3;
        NSCAssert(Negotiate(server) && server.calls == 3, @"Wait for client acknowledgements");
        server.calls = 0; server.readyAfter = 100;
        NSCAssert(!Negotiate(server) && server.calls == 20, @"Refusal must preserve clients after bounded retries");
        server.calls = 0; server.fails = YES;
        NSCAssert(!Negotiate(server) && server.calls == 1, @"Exceptions must not force termination");
        puts("native lifecycle: async acknowledgement, bounded refusal and exception checks passed");
    }
    return 0;
}
