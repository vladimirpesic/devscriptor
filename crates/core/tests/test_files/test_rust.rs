/**
 * Comprehensive Rust test file containing all entities and relationships.
 */

use std::collections::HashMap;

// Enum
enum Color {
    Red,
    Green,
    Blue,
    Custom { r: u8, g: u8, b: u8 },
}

// Enum with discriminant
enum Priority {
    Low = 1,
    Medium = 2,
    High = 3,
}

// Option (built-in enum)
fn divide(a: f64, b: f64) -> Option<f64> {
    if b == 0.0 {
        None
    } else {
        Some(a / b)
    }
}

// Result (built-in enum)
fn parse_number(s: &str) -> Result<i32, std::num::ParseIntError> {
    s.parse::<i32>()
}

// Struct
struct Person {
    name: String,
    age: u32,
    email: String,
}

impl Person {
    fn new(name: String, age: u32, email: String) -> Self {
        Person { name, age, email }
    }
    
    fn greet(&self) -> String {
        format!("Hello, I'm {}", self.name)
    }
}

// Tuple struct
struct Point(i32, i32);

// Unit struct
struct Marker;

// Trait (interface)
trait Shape {
    fn area(&self) -> f64;
    fn perimeter(&self) -> f64;
}

// Trait with default implementation
trait Drawable {
    fn draw(&self) {
        println!("Drawing shape");
    }
}

// Implement trait for struct
struct Circle {
    radius: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }
    
    fn perimeter(&self) -> f64 {
        2.0 * std::f64::consts::PI * self.radius
    }
}

impl Drawable for Circle {
    fn draw(&self) {
        println!("Drawing circle with radius {}", self.radius);
    }
}

impl Circle {
    fn new(radius: f64) -> Self {
        Circle { radius }
    }
}

// Generic struct
struct Container<T> {
    items: Vec<T>,
}

impl<T> Container<T> {
    fn new() -> Self {
        Container { items: Vec::new() }
    }
    
    fn add(&mut self, item: T) {
        self.items.push(item);
    }
    
    fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index)
    }
    
    fn len(&self) -> usize {
        self.items.len()
    }
}

// Generic function
fn add<T: std::ops::Add<Output = T>>(a: T, b: T) -> T {
    a + b
}

// Function with lifetime
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() > y.len() {
        x
    } else {
        y
    }
}

// Closure
fn demonstrate_closures() {
    let x = 10;
    
    // Closure that captures by reference
    let add_x = |y| x + y;
    println!("Add x: {}", add_x(5));
    
    // Closure that captures by value
    let mut counter = 0;
    let mut increment = || {
        counter += 1;
        counter
    };
    println!("Counter: {}", increment());
    
    // Closure as parameter
    let apply = |f: &dyn Fn(i32) -> i32, value| f(value);
    let result = apply(&|x| x * 2, 5);
    println!("Applied: {}", result);
}

// Iterator
fn demonstrate_iterators() {
    let numbers = vec![1, 2, 3, 4, 5];
    
    // Iterator chain
    let sum: i32 = numbers.iter()
        .map(|x| x * 2)
        .filter(|x| x > 5)
        .sum();
    
    println!("Iterator sum: {}", sum);
    
    // For loop with iterator
    for num in numbers.iter() {
        println!("Number: {}", num);
    }
}

// Higher-order function
fn apply_operation<F>(a: i32, b: i32, op: F) -> i32
where
    F: Fn(i32, i32) -> i32,
{
    op(a, b)
}

// Method with self types
struct Counter {
    count: u32,
}

impl Counter {
    fn new() -> Self {
        Counter { count: 0 }
    }
    
    // Consumes self
    fn into_inner(self) -> u32 {
        self.count
    }
    
    // Borrows self immutably
    fn get(&self) -> u32 {
        self.count
    }
    
    // Borrows self mutably
    fn increment(&mut self) {
        self.count += 1;
    }
}

// Associated constants
trait MathConstants {
    const PI: f64 = 3.14159;
    const E: f64 = 2.71828;
}

struct Calculator;
impl MathConstants for Calculator {}

// Macro
macro_rules! greet {
    ($name:expr) => {
        println!("Hello, {}!", $name);
    };
}

// Derive macro
#[derive(Debug)]
struct Debuggable {
    value: i32,
}

// Main function
fn main() {
    // Enum usage
    let color = Color::Red;
    let priority = Priority::High;
    
    // Option and Result
    match divide(10.0, 2.0) {
        Some(result) => println!("Division: {}", result),
        None => println!("Cannot divide"),
    }
    
    match parse_number("42") {
        Ok(num) => println!("Parsed: {}", num),
        Err(e) => println!("Error: {}", e),
    }
    
    // Struct usage
    let person = Person::new(
        String::from("Alice"),
        25,
        String::from("alice@example.com"),
    );
    println!("{}", person.greet());
    
    // Circle implementing trait
    let circle = Circle::new(5.0);
    println!("Area: {}", circle.area());
    println!("Perimeter: {}", circle.perimeter());
    circle.draw();
    
    // Generic container
    let mut container: Container<i32> = Container::new();
    container.add(10);
    container.add(20);
    println!("Container length: {}", container.len());
    
    // Generic function
    println!("Add ints: {}", add(5, 3));
    println!("Add floats: {}", add(5.5, 3.3));
    
    // Closures
    demonstrate_closures();
    
    // Iterators
    demonstrate_iterators();
    
    // Higher-order function
    let result = apply_operation(10, 5, |a, b| a * b);
    println!("Applied: {}", result);
    
    // Counter with different self types
    let mut counter = Counter::new();
    counter.increment();
    println!("Counter: {}", counter.get());
    let inner = counter.into_inner();
    println!("Inner: {}", inner);
    
    // Associated constants
    println!("PI: {}", Calculator::PI);
    println!("E: {}", Calculator::E);
    
    // Macro
    greet!("World");
    
    // Derive
    let debug = Debuggable { value: 42 };
    println!("Debug: {:?}", debug);
}
