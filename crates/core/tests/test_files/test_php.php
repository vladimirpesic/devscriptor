<?php
/**
 * Comprehensive PHP test file containing all entities and relationships.
 */

// Namespace
namespace App\Models;

use App\Services;
use Closure;

// Interface
interface Shape {
    public function area(): float;
    public function perimeter(): float;
}

interface Drawable {
    public function draw(): void;
}

// Trait
trait Loggable {
    public function log(string $message): void {
        echo "Log: {$message}\n";
    }
}

// Abstract class
abstract class AbstractShape implements Shape {
    protected string $name;
    
    public function __construct(string $name) {
        $this->name = $name;
    }
    
    public function getName(): string {
        return $this->name;
    }
    
    public function describe(): void {
        echo "Shape: {$this->name}\n";
    }
}

// Class implementing interfaces and using traits
class Circle extends AbstractShape implements Drawable {
    use Loggable;
    
    private float $radius;
    
    public function __construct(string $name, float $radius) {
        parent::__construct($name);
        $this->radius = $radius;
    }
    
    public function area(): float {
        return M_PI * $this->radius * $this->radius;
    }
    
    public function perimeter(): float {
        return 2 * M_PI * $this->radius;
    }
    
    public function draw(): void {
        echo "Drawing circle: {$this->name}\n";
        $this->log("Circle drawn");
    }
}

// Enum (PHP 8.1+)
enum Color: string {
    case RED = 'red';
    case GREEN = 'green';
    case BLUE = 'blue';
    
    public function description(): string {
        return "This is {$this->value}";
    }
}

enum Priority: int {
    case LOW = 1;
    case MEDIUM = 2;
    case HIGH = 3;
}

// Backed enum
enum Status {
    case ACTIVE;
    case INACTIVE;
    case PENDING;
    
    public function label(): string {
        return match($this) {
            Status::ACTIVE => 'Active',
            Status::INACTIVE => 'Inactive',
            Status::PENDING => 'Pending',
        };
    }
}

// Generic-like class using PHP type system
class Container {
    private array $items = [];
    
    public function add($item): void {
        $this->items[] = $item;
    }
    
    public function get(int $index): mixed {
        return $this->items[$index] ?? null;
    }
    
    public function count(): int {
        return count($this->items);
    }
}

// Class with properties and methods
class Person {
    public string $name;
    public int $age;
    private string $email;
    protected static int $count = 0;
    
    public function __construct(string $name, int $age, string $email) {
        $this->name = $name;
        $this->age = $age;
        $this->email = $email;
        self::$count++;
    }
    
    public function greet(): void {
        echo "Hello, I'm {$this->name}\n";
    }
    
    public function getEmail(): string {
        return $this->email;
    }
    
    public function setEmail(string $email): void {
        $this->email = $email;
    }
    
    public static function createDefault(): self {
        return new self('Unknown', 0, 'unknown@example.com');
    }
    
    public static function getCount(): int {
        return self::$count;
    }
}

// Anonymous class (PHP 7+)
function createAnonymous() {
    return new class {
        public function sayHello() {
            echo "Hello from anonymous class\n";
        }
    };
}

// Arrow function (PHP 7.4+)
$add = fn($a, $b) => $a + $b;

// Closure
$multiplier = function(int $factor): Closure {
    return function(int $x) use ($factor): int {
        return $x * $factor;
    };
};

// Function with union types (PHP 8.0+)
function processValue(int|float|string $value): string {
    return (string)$value;
}

// Function with intersection types (PHP 8.1+)
function processShape(Shape&Drawable $shape): void {
    $shape->draw();
    echo "Area: {$shape->area()}\n";
}

// Function with null and nullable types
function findUser(?int $id): ?Person {
    if ($id === 1) {
        return new Person('John', 30, 'john@example.com');
    }
    return null;
}

// Match expression (PHP 8.0+)
function matchPattern(mixed $value): string {
    return match($value) {
        is_int($value) => "Integer: {$value}",
        is_string($value) => "String: {$value}",
        is_array($value) => "Array with " . count($value) . " items",
        default => "Unknown type",
    };
}

// Attributes (PHP 8.0+)
#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_METHOD)]
class CustomAttribute {
    public function __construct(
        public string $description = '',
        public int $priority = 0
    ) {}
}

#[Custom('Person class', priority: 1)]
class AnnotatedPerson {
    #[Custom('Greet method')]
    public function greet(): void {
        echo "Hello from annotated person\n";
    }
}

// Generator function
function numberGenerator(int $count): Generator {
    for ($i = 0; $i < $count; $i++) {
        yield $i;
    }
}

// Async-like using promises (conceptual)
class Promise {
    private $callback;
    
    public function __construct(callable $callback) {
        $this->callback = $callback;
    }
    
    public function then(callable $callback): self {
        return new self(function() use ($callback) {
            return $callback($this->resolve());
        });
    }
    
    private function resolve() {
        return ($this->callback)();
    }
    
    public function await() {
        return $this->resolve();
    }
}

// Interface with constants
interface Constants {
    public const MAX_SIZE = 100;
    public const MIN_SIZE = 1;
}

// Final class
final class FinalClass {
    public function doSomething(): void {
        echo "Doing something\n";
    }
}

// Readonly class (PHP 8.2+)
readonly class ReadonlyData {
    public string $name;
    public int $value;
    
    public function __construct(string $name, int $value) {
        $this->name = $name;
        $this->value = $value;
    }
}

// Magic methods
class MagicMethods {
    private array $data = [];
    
    public function __get(string $name): mixed {
        return $this->data[$name] ?? null;
    }
    
    public function __set(string $name, mixed $value): void {
        $this->data[$name] = $value;
    }
    
    public function __call(string $name, array $arguments): mixed {
        echo "Calling method: {$name}\n";
        return null;
    }
    
    public static function __callStatic(string $name, array $arguments): mixed {
        echo "Calling static method: {$name}\n";
        return null;
    }
    
    public function __toString(): string {
        return "MagicMethods instance";
    }
    
    public function __invoke(): void {
        echo "Object invoked as function\n";
    }
}

// Exception handling
class CustomException extends Exception {}

function riskyOperation(): void {
    throw new CustomException("Something went wrong");
}

// Main execution
function main(): void {
    // Namespace usage
    $circle = new Circle("MyCircle", 5.0);
    echo "Area: {$circle->area()}\n";
    echo "Perimeter: {$circle->perimeter()}\n";
    $circle->draw();
    
    // Enum usage
    $color = Color::RED;
    echo "Color: {$color->description()}\n";
    
    $priority = Priority::HIGH;
    echo "Priority: {$priority->value}\n";
    
    $status = Status::ACTIVE;
    echo "Status: {$status->label()}\n";
    
    // Container
    $container = new Container();
    $container->add(10);
    $container->add(20);
    echo "Container count: {$container->count()}\n";
    
    // Person
    $person = new Person("Alice", 25, "alice@example.com");
    $person->greet();
    echo "Person count: " . Person::getCount() . "\n";
    
    // Anonymous class
    $anonymous = createAnonymous();
    $anonymous->sayHello();
    
    // Arrow function
    $sum = $add(5, 3);
    echo "Sum: {$sum}\n";
    
    // Closure
    $double = $multiplier(2);
    echo "Double of 5: {$double(5)}\n";
    
    // Union type
    echo "Process value: " . processValue(42) . "\n";
    
    // Nullable type
    $user = findUser(1);
    if ($user !== null) {
        $user->greet();
    }
    
    // Match expression
    echo "Match int: " . matchPattern(42) . "\n";
    echo "Match string: " . matchPattern("hello") . "\n";
    echo "Match array: " . matchPattern([1, 2, 3]) . "\n";
    
    // Generator
    $gen = numberGenerator(5);
    foreach ($gen as $num) {
        echo "Number: {$num}\n";
    }
    
    // Magic methods
    $magic = new MagicMethods();
    $magic->property = "value";
    echo "Magic property: {$magic->property}\n";
    $magic->unknownMethod();
    echo "Magic string: {$magic}\n";
    $magic();
    
    // Readonly class
    $readonly = new ReadonlyData("test", 42);
    echo "Readonly: {$readonly->name}, {$readonly->value}\n";
}

main();
