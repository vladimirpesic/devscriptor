// Comprehensive Objective-C test file

#import <Foundation/Foundation.h>

// Protocol definitions
@protocol Printable <NSObject>
@required
- (NSString *)printDescription;
@optional
- (void)printDetails;
@end

@protocol DataSource <NSObject>
@required
- (NSInteger)numberOfItems;
- (id)itemAtIndex:(NSInteger)index;
@end

// Enum definitions
typedef NS_ENUM(NSInteger, Color) {
    ColorRed = 0,
    ColorGreen,
    ColorBlue,
    ColorYellow
};

typedef NS_OPTIONS(NSInteger, Options) {
    OptionNone = 0,
    OptionA = 1 << 0,
    OptionB = 1 << 1,
    OptionC = 1 << 2
};

// Block definitions
typedef void (^CompletionBlock)(BOOL success, NSError *error);
typedef NSInteger (^CalculationBlock)(NSInteger a, NSInteger b);
typedef NSString* (^StringBlock)(NSString *input);

// Category declaration
@interface NSString (Utilities)
- (BOOL)isUppercase;
- (NSString *)reverseString;
@end

// Extension declaration
@interface TestObject ()
@property (nonatomic, strong) NSString *internalProperty;
- (void)privateMethod;
@end

// Main class declaration
@interface TestObject : NSObject <Printable, DataSource>

// Properties
@property (nonatomic, assign) NSInteger integerValue;
@property (nonatomic, strong) NSString *name;
@property (nonatomic, copy) NSString *copyString;
@property (nonatomic, weak) id<DataSource> weakDelegate;
@property (nonatomic, readonly) BOOL readOnlyProperty;
@property (class, nonatomic, readonly) NSString *classProperty;

// Instance methods
- (instancetype)initWithName:(NSString *)name;
- (void)performAction;
- (NSInteger)calculateSum:(NSInteger)a with:(NSInteger)b;
- (void)executeWithCompletion:(CompletionBlock)completion;
- (NSString *)description;

// Class methods
+ (instancetype)sharedInstance;
+ (NSString *)classProperty;
+ (TestObject *)objectWithName:(NSString *)name;

@end

// Implementation
@implementation TestObject {
    NSInteger _privateInstanceVariable;
    NSString *_internalString;
}

@synthesize readOnlyProperty = _readOnlyProperty;

#pragma mark - Initialization

- (instancetype)init {
    self = [super init];
    if (self) {
        _integerValue = 0;
        _name = @"";
        _internalString = @"internal";
    }
    return self;
}

- (instancetype)initWithName:(NSString *)name {
    self = [super init];
    if (self) {
        _name = [name copy];
        _integerValue = 0;
    }
    return self;
}

#pragma mark - Public Methods

- (void)performAction {
    NSLog(@"Performing action for %@", self.name);
    
    if ([self.weakDelegate respondsToSelector:@selector(numberOfItems)]) {
        NSInteger count = [self.weakDelegate numberOfItems];
        NSLog(@"Delegate reports %ld items", (long)count);
    }
}

- (NSInteger)calculateSum:(NSInteger)a with:(NSInteger)b {
    return a + b;
}

- (void)executeWithCompletion:(CompletionBlock)completion {
    dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
        // Simulate async work
        [NSThread sleepForTimeInterval:0.1];
        
        BOOL success = YES;
        NSError *error = nil;
        
        dispatch_async(dispatch_get_main_queue(), ^{
            if (completion) {
                completion(success, error);
            }
        });
    });
}

- (NSString *)description {
    return [NSString stringWithFormat:@"TestObject: name=%@, value=%ld", 
            self.name, (long)self.integerValue];
}

#pragma mark - Printable Protocol

- (NSString *)printDescription {
    return [self description];
}

- (void)printDetails {
    NSLog(@"Details: %@", self.internalProperty);
}

#pragma mark - DataSource Protocol

- (NSInteger)numberOfItems {
    return self.integerValue;
}

- (id)itemAtIndex:(NSInteger)index {
    if (index < self.integerValue) {
        return [NSString stringWithFormat:@"Item %ld", (long)index];
    }
    return nil;
}

#pragma mark - Private Methods

- (void)privateMethod {
    NSLog(@"Private method called");
}

#pragma mark - Class Methods

+ (instancetype)sharedInstance {
    static TestObject *instance = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        instance = [[TestObject alloc] init];
    });
    return instance;
}

+ (NSString *)classProperty {
    return @"TestClass";
}

+ (TestObject *)objectWithName:(NSString *)name {
    return [[TestObject alloc] initWithName:name];
}

@end

// Category implementation
@implementation NSString (Utilities)

- (BOOL)isUppercase {
    return [self isEqualToString:[self uppercaseString]];
}

- (NSString *)reverseString {
    NSMutableString *reversed = [NSMutableString stringWithCapacity:[self length]];
    [self enumerateSubstringsInRange:NSMakeRange(0, [self length])
                            options:(NSStringEnumerationReverse | NSStringEnumerationByComposedCharacterSequences)
                         usingBlock:^(NSString *substring, NSRange substringRange, NSRange enclosingRange, BOOL *stop) {
                             [reversed appendString:substring];
                         }];
    return reversed;
}

@end

// Subclass
@interface SpecialTestObject : TestObject

@property (nonatomic, strong) NSMutableArray *items;

- (void)addItem:(id)item;
- (void)removeItemAtIndex:(NSInteger)index;
- (NSArray *)allItems;

@end

@implementation SpecialTestObject

- (instancetype)init {
    self = [super init];
    if (self) {
        _items = [NSMutableArray array];
    }
    return self;
}

- (void)addItem:(id)item {
    [self.items addObject:item];
}

- (void)removeItemAtIndex:(NSInteger)index {
    if (index >= 0 && index < [self.items count]) {
        [self.items removeObjectAtIndex:index];
    }
}

- (NSArray *)allItems {
    return [self.items copy];
}

@end

// Class with blocks
@interface BlockTester : NSObject

- (void)testBlocks;
- (NSInteger)performCalculation:(CalculationBlock)block withA:(NSInteger)a andB:(NSInteger)b;
- (void)chainBlocksWithCompletion:(CompletionBlock)completion;

@end

@implementation BlockTester

- (void)testBlocks {
    // Simple block
    void (^simpleBlock)(void) = ^{
        NSLog(@"Simple block executed");
    };
    simpleBlock();
    
    // Block with parameters
    NSInteger (^addBlock)(NSInteger, NSInteger) = ^(NSInteger a, NSInteger b) {
        return a + b;
    };
    
    NSInteger result = addBlock(5, 3);
    NSLog(@"Block result: %ld", (long)result);
    
    // Block that captures variables
    NSInteger multiplier = 2;
    NSInteger (^multiplyBlock)(NSInteger) = ^(NSInteger value) {
        return value * multiplier;
    };
    
    NSLog(@"Multiplied result: %ld", (long)multiplyBlock(10));
}

- (NSInteger)performCalculation:(CalculationBlock)block withA:(NSInteger)a andB:(NSInteger)b {
    if (block) {
        return block(a, b);
    }
    return 0;
}

- (void)chainBlocksWithCompletion:(CompletionBlock)completion {
    dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
        // First operation
        sleep(1);
        
        dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_LOW, 0), ^{
            // Second operation
            sleep(1);
            
            dispatch_async(dispatch_get_main_queue(), ^{
                if (completion) {
                    completion(YES, nil);
                }
            });
        });
    });
}

@end

// Class with KVC/KVO
@interface KVOTester : NSObject

@property (nonatomic, strong) NSString *observableValue;

- (void)observeValue:(NSString *)keyPath;

@end

@implementation KVOTester

- (instancetype)init {
    self = [super init];
    if (self) {
        _observableValue = @"initial";
    }
    return self;
}

- (void)observeValue:(NSString *)keyPath {
    [self addObserver:self forKeyPath:keyPath options:NSKeyValueObservingOptionNew context:nil];
}

- (void)observeValueForKeyPath:(NSString *)keyPath ofObject:(id)object change:(NSDictionary<NSKeyValueChangeKey,id> *)change context:(void *)context {
    NSLog(@"Value changed: %@", change[NSKeyValueChangeNewKey]);
}

- (void)dealloc {
    [self removeObserver:self forKeyPath:@"observableValue"];
}

@end

// Exception handling class
@interface ExceptionTester : NSObject

- (void)testExceptionHandling;
- (void) riskyOperation;

@end

@implementation ExceptionTester

- (void)testExceptionHandling {
    @try {
        [self riskyOperation];
    } @catch (NSException *exception) {
        NSLog(@"Caught exception: %@", exception.reason);
    } @finally {
        NSLog(@"Finally block executed");
    }
}

- (void)riskyOperation {
    @throw [NSException exceptionWithName:@"TestException"
                                   reason:@"Testing exception handling"
                                 userInfo:nil];
}

@end

// Memory management example
@interface MemoryTester : NSObject

@property (nonatomic, strong) NSMutableArray *strongArray;
@property (nonatomic, weak) id weakReference;
@property (nonatomic, unsafe_unretained) id unsafeReference;

- (void)demonstrateMemoryManagement;

@end

@implementation MemoryTester

- (instancetype)init {
    self = [super init];
    if (self) {
        _strongArray = [NSMutableArray array];
    }
    return self;
}

- (void)demonstrateMemoryManagement {
    // Strong reference
    NSString *strongString = [[NSString alloc] initWithFormat:@"Strong string %d", 1];
    [self.strongArray addObject:strongString];
    
    // Weak reference
    __weak NSString *weakString = strongString;
    
    // Autorelease pool
    @autoreleasepool {
        NSString *tempString = [NSString stringWithFormat:@"Temp string"];
        NSLog(@"%@", tempString);
    }
    
    // Using weak reference
    if (weakString) {
        NSLog(@"Weak string still exists: %@", weakString);
    }
}

@end

// Notification center example
@interface NotificationTester : NSObject

- (void)registerForNotifications;
- (void)postNotification;

@end

@implementation NotificationTester

- (void)registerForNotifications {
    [[NSNotificationCenter defaultCenter] addObserver:self
                                             selector:@selector(handleNotification:)
                                                 name:@"TestNotification"
                                               object:nil];
}

- (void)handleNotification:(NSNotification *)notification {
    NSLog(@"Received notification: %@", notification.userInfo);
}

- (void)postNotification {
    NSDictionary *userInfo = @{@"key": @"value"};
    [[NSNotificationCenter defaultCenter] postNotificationName:@"TestNotification"
                                                        object:self
                                                      userInfo:userInfo];
}

- (void)dealloc {
    [[NSNotificationCenter defaultCenter] removeObserver:self];
}

@end

// GCD example
@interface GCDTester : NSObject

- (void)testDispatchQueue;
- (void)testDispatchGroup;
- (void)testDispatchSemaphore;

@end

@implementation GCDTester

- (void)testDispatchQueue {
    // Main queue
    dispatch_async(dispatch_get_main_queue(), ^{
        NSLog(@"Executing on main queue");
    });
    
    // Global queue
    dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_HIGH, 0), ^{
        NSLog(@"Executing on high priority queue");
    });
    
    // Custom queue
    dispatch_queue_t customQueue = dispatch_queue_create("com.test.custom", DISPATCH_QUEUE_CONCURRENT);
    dispatch_async(customQueue, ^{
        NSLog(@"Executing on custom concurrent queue");
    });
}

- (void)testDispatchGroup {
    dispatch_group_t group = dispatch_group_create();
    
    dispatch_group_enter(group);
    dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
        NSLog(@"Task 1");
        dispatch_group_leave(group);
    });
    
    dispatch_group_enter(group);
    dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
        NSLog(@"Task 2");
        dispatch_group_leave(group);
    });
    
    dispatch_group_notify(group, dispatch_get_main_queue(), ^{
        NSLog(@"All tasks completed");
    });
}

- (void)testDispatchSemaphore {
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(2);
    
    for (int i = 0; i < 5; i++) {
        dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), ^{
            dispatch_semaphore_wait(semaphore, DISPATCH_TIME_FOREVER);
            NSLog(@"Task %d started", i);
            sleep(1);
            NSLog(@"Task %d completed", i);
            dispatch_semaphore_signal(semaphore);
        });
    }
}

@end

// Protocol with optional methods
@protocol OptionalProtocol <NSObject>
@required
- (void)requiredMethod;
@optional
- (void)optionalMethod;
- (void)anotherOptionalMethod;
@end

// Implementation with optional protocol
@interface OptionalProtocolTester : NSObject <OptionalProtocol>

@end

@implementation OptionalProtocolTester

- (void)requiredMethod {
    NSLog(@"Required method called");
}

- (void)optionalMethod {
    NSLog(@"Optional method called");
}

@end

// Main function
int main(int argc, const char * argv[]) {
    @autoreleasepool {
        // Test basic class
        TestObject *obj = [[TestObject alloc] initWithName:@"Test"];
        [obj performAction];
        NSInteger sum = [obj calculateSum:10 with:20];
        NSLog(@"Sum: %ld", (long)sum);
        
        // Test subclass
        SpecialTestObject *special = [[SpecialTestObject alloc] init];
        [special addItem:@"Item 1"];
        [special addItem:@"Item 2"];
        NSLog(@"All items: %@", [special allItems]);
        
        // Test blocks
        BlockTester *blockTester = [[BlockTester alloc] init];
        [blockTester testBlocks];
        
        // Test category
        NSString *testString = @"Hello World";
        NSLog(@"Reversed: %@", [testString reverseString]);
        
        // Test protocol
        id<Printable> printable = obj;
        NSLog(@"%@", [printable printDescription]);
        
        // Test singleton
        TestObject *shared1 = [TestObject sharedInstance];
        TestObject *shared2 = [TestObject sharedInstance];
        NSLog(@"Singleton test: %@", shared1 == shared2 ? @"Same" : @"Different");
        
        // Test optional protocol
        OptionalProtocolTester *optionalTester = [[OptionalProtocolTester alloc] init];
        [optionalTester requiredMethod];
        if ([optionalTester respondsToSelector:@selector(optionalMethod)]) {
            [optionalTester optionalMethod];
        }
    }
    return 0;
}
