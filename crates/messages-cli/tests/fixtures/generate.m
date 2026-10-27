// Recreate the synthetic fixtures on macOS:
// clang -fobjc-arc -framework Foundation generate.m -o /tmp/messages-fixtures
// /tmp/messages-fixtures <this-directory>
#import <Foundation/Foundation.h>

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc != 2) {
            return 1;
        }
        NSString *directory = [NSString stringWithUTF8String:argv[1]];
        NSMutableString *longText = [[NSMutableString alloc] init];
        NSString *part = [[NSString alloc] initWithBytes:"A\xf0\x9f\x99\x82\0Z" length:7 encoding:NSUTF8StringEncoding];
        for (NSUInteger i = 0; i < 400; i++) {
            [longText appendString:part];
        }
        NSDictionary *cases = @{@"plain": @"Hello from Messages 👋", @"long": longText, @"empty": @"", @"mutable": @"Styled text"};
        for (NSString *name in cases) {
            NSAttributedString *body = [[NSAttributedString alloc] initWithString:cases[name]];
            if ([name isEqualToString:@"mutable"]) {
                NSMutableAttributedString *mutable = [[NSMutableAttributedString alloc] initWithString:cases[name]];
                [mutable addAttribute:@"ExampleAttribute" value:@"ExampleValue" range:NSMakeRange(0, 6)];
                body = mutable;
            }
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            NSData *data = [NSArchiver archivedDataWithRootObject:body];
#pragma clang diagnostic pop
            NSString *path = [directory stringByAppendingPathComponent:[name stringByAppendingString:@".typedstream"]];
            if (![data writeToFile:path atomically:YES]) {
                return 1;
            }
        }
    }
    return 0;
}
