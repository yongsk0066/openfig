#import <AppKit/AppKit.h>
#include <unistd.h>

static NSString *StatePath;
static NSString *CommandPath;
static NSUInteger Calls;
static NSUInteger LastIndex;
static NSUInteger Mode;

@interface FixtureView : NSView <NSTextInputClient>
- (NSRect)caret;
@end
@implementation FixtureView
- (BOOL)acceptsFirstResponder { return YES; }
- (void)keyDown:(NSEvent *)event { [self interpretKeyEvents:@[event]]; }
- (void)doCommandBySelector:(SEL)selector { (void)selector; }
- (NSRect)caret {
    NSRect local = NSMakeRect(30 + 71 * Mode, 120 + 19 * Mode, 0, 22);
    return [self.window convertRectToScreen:[self convertRect:local toView:nil]];
}
- (NSRect)firstRectForCharacterRange:(NSRange)range actualRange:(NSRangePointer)actual {
    Calls++; LastIndex = range.location;
    if (actual) *actual = NSMakeRange(0, 0);
    return self.caret;
}
- (void)insertText:(id)text replacementRange:(NSRange)range { (void)text; (void)range; }
- (void)setMarkedText:(id)text selectedRange:(NSRange)selected replacementRange:(NSRange)replacement {
    (void)text; (void)selected; (void)replacement;
}
- (void)unmarkText {}
- (BOOL)hasMarkedText { return NO; }
- (NSRange)markedRange { return NSMakeRange(NSNotFound, 0); }
- (NSRange)selectedRange { return NSMakeRange(0, 0); }
- (NSArray *)validAttributesForMarkedText { return @[]; }
- (NSAttributedString *)attributedSubstringForProposedRange:(NSRange)range actualRange:(NSRangePointer)actual {
    (void)range; (void)actual; return nil;
}
- (NSUInteger)characterIndexForPoint:(NSPoint)point { (void)point; return 0; }
@end

int main(int argc, const char *argv[]) {
    if (argc != 3) return 2;
    @autoreleasepool {
        StatePath = @(argv[1]); CommandPath = @(argv[2]);
        NSApplication *app = NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(180, 240, 640, 400)
            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskResizable backing:NSBackingStoreBuffered defer:NO];
        window.title = @"OpenFig IMK contract fixture";
        FixtureView *view = [[FixtureView alloc] initWithFrame:NSMakeRect(0, 0, 640, 400)];
        window.contentView = view;
        [window makeKeyAndOrderFront:nil];
        [window makeFirstResponder:view];
        [app finishLaunching];
        [app activate];
        [view.inputContext activate];
        [NSTimer scheduledTimerWithTimeInterval:0.05 repeats:YES block:^(NSTimer *timer) {
            (void)timer;
            NSData *data = [NSData dataWithContentsOfFile:CommandPath];
            NSDictionary *command = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
            if ([command[@"quit"] boolValue]) { [app terminate:nil]; return; }
            NSUInteger mode = [command[@"mode"] unsignedIntegerValue];
            if (Mode != mode) {
                Mode = mode;
                [window setFrameOrigin:NSMakePoint(180 + 29 * mode, 240 + 13 * mode)];
                [view.inputContext invalidateCharacterCoordinates];
            }
            NSRect rect = view.caret;
            CGFloat top = NSMaxY(NSScreen.screens.firstObject.frame);
            NSDictionary *state = @{@"pid": @(getpid()), @"window": @(window.windowNumber),
                @"front": @(NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier),
                @"expected": @[@(rect.origin.x), @(top - NSMaxY(rect)), @(rect.size.width), @(rect.size.height)],
                @"calls": @(Calls), @"index": @(LastIndex), @"mode": @(Mode)};
            [[NSJSONSerialization dataWithJSONObject:state options:0 error:nil] writeToFile:StatePath atomically:YES];
        }];
        [app run];
    }
    return 0;
}
