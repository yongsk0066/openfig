#define FIG_IME_FIXTURE 1
#define main FigInputMethodMain
#include "../main.m"
#undef main

int main(void) {
    @autoreleasepool {
        NSArray *rect = QuartzRect(NSMakeRect(-1300, 450, 0, 20), 900);
        NSCAssert(([rect isEqual:@[@(-1300), @430, @0, @20]]), @"Retina points must not be rescaled");
        NSCAssert(([QuartzRect(NSMakeRect(100, 1000, 1, 18), 900) isEqual:@[@100, @(-118), @1, @18]]), @"Display above primary");
        NSCAssert(QuartzRect(NSMakeRect(0, 0, 0, 0), 900) == nil, @"Missing rectangle");
        NSCAssert(QuartzRect(NSMakeRect(NAN, 0, 1, 18), 900) == nil, @"NaN");
        NSCAssert(QuartzRect(NSMakeRect(0, 0, -1, 18), 900) == nil, @"Negative width");
        NSCAssert(!IsSupportedTarget(@"com.mitchellh.ghostty"), @"Fixture helper must never query Ghostty");
        for (NSNumber *attached in @[@NO, @YES]) {
            NSData *json = [NSJSONSerialization dataWithJSONObject:@{@"attachment": AttachmentState(YES, attached.boolValue)} options:0 error:nil];
            NSString *wire = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
            NSCAssert([wire isEqual:(attached.boolValue ? @"{\"attachment\":true}" : @"{\"attachment\":false}")], @"Rust requires a JSON boolean, not 0/1");
        }
        NSCAssert(AttachmentState(NO, YES) == NSNull.null, @"Background is not a missing client");
        puts("native geometry/protocol: 9 checks passed");
    }
    return 0;
}
