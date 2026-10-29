// Comprehensive Swift test file

import Foundation

// User struct
struct User {
    let id: Int
    let username: String
    let email: String
    let age: Int
    let isActive: Bool
    
    init(id: Int, username: String, email: String, age: Int, isActive: Bool = true) {
        self.id = id
        self.username = username
        self.email = email
        self.age = age
        self.isActive = isActive
    }
    
    var description: String {
        "User(id: \(id), username: \(username), email: \(email), age: \(age), isActive: \(isActive))"
    }
    
    func formatInfo() -> String {
        """
        User Information:
        ID: \(id)
        Name: \(username)
        Email: \(email)
        Age: \(age)
        Active: \(isActive)
        """
    }
}

// Status enum
enum Status {
    case pending
    case processing
    case completed
    case cancelled
}

// Utility functions
func isAdult(_ age: Int) -> Bool {
    age >= 18
}

func findUser(_ users: [User], id: Int) -> User? {
    users.first { $0.id == id }
}

func filterActiveUsers(_ users: [User]) -> [User] {
    users.filter { $0.isActive }
}

func mapUserNames(_ users: [User]) -> [String] {
    users.map { $0.username }
}

func sumAges(_ users: [User]) -> Int {
    users.reduce(0) { $0 + $1.age }
}

func averageAge(_ users: [User]) -> Double {
    guard !users.isEmpty else { return 0.0 }
    return Double(sumAges(users)) / Double(users.count)
}

// Shape protocol
protocol Shape {
    func area() -> Double
    func describe() -> String
}

struct Circle: Shape {
    let radius: Double
    
    func area() -> Double {
        Double.pi * radius * radius
    }
    
    func describe() -> String {
        "Circle with radius \(radius)"
    }
}

struct Rectangle: Shape {
    let width: Double
    let height: Double
    
    func area() -> Double {
        width * height
    }
    
    func describe() -> String {
        "Rectangle \(width)x\(height)"
    }
}

struct Triangle: Shape {
    let base: Double
    let height: Double
    let side3: Double
    
    func area() -> Double {
        0.5 * base * height
    }
    
    func describe() -> String {
        "Triangle \(base)-\(height)-\(side3)"
    }
}

// Recursion
func factorial(_ n: Int) -> Int {
    n <= 1 ? 1 : n * factorial(n - 1)
}

func fibonacci(_ n: Int) -> Int {
    n <= 1 ? n : fibonacci(n - 1) + fibonacci(n - 2)
}

// Higher-order functions
func mapList<T, U>(_ list: [T], _ fn: (T) -> U) -> [U] {
    list.map(fn)
}

func filterList<T>(_ list: [T], _ predicate: (T) -> Bool) -> [T] {
    list.filter(predicate)
}

func reduceList<T>(_ list: [T], _ initial: T, _ fn: (T, T) -> T) -> T {
    list.reduce(initial, fn)
}

// Error handling
enum DivisionError: Error {
    case divisionByZero
}

func safeDivide(_ numerator: Double, _ denominator: Double) -> Result<Double, DivisionError> {
    guard denominator != 0 else {
        return .failure(.divisionByZero)
    }
    return .success(numerator / denominator)
}

// Extensions
extension String {
    func capitalized() -> String {
        self.capitalized
    }
    
    func reversed() -> String {
        String(self.reversed())
    }
}

// Main function
func main() {
    print("=== Swift Test Program ===\n")
    
    // Create sample users
    let users: [User] = [
        User(id: 1, username: "Alice", email: "alice@example.com", age: 25),
        User(id: 2, username: "Bob", email: "bob@example.com", age: 30),
        User(id: 3, username: "Charlie", email: "charlie@example.com", age: 17, isActive: false)
    ]
    
    // Print user information
    print("Users:")
    for user in users {
        print(user.formatInfo())
        print("")
    }
    
    // Filter active users
    print("Active Users:")
    let activeUsers = filterActiveUsers(users)
    for user in activeUsers {
        print(user.username)
    }
    print("")
    
    // Adult users
    print("Adult Users:")
    for user in users {
        if isAdult(user.age) {
            print(user.username)
        }
    }
    print("")
    
    // Find specific user
    print("Finding user with ID 2:")
    if let foundUser = findUser(users, id: 2) {
        print("Found: \(foundUser.username)")
    } else {
        print("User not found")
    }
    print("")
    
    // Calculations
    print("Calculations:")
    print("Total ages: \(sumAges(users))")
    print(String(format: "Average age: %.2f", averageAge(users)))
    print("")
    
    // Factorial and Fibonacci
    print("Factorial 5: \(factorial(5))")
    print("Fibonacci 10: \(fibonacci(10))")
    print("")
    
    // Shapes
    print("Shapes:")
    let circle = Circle(radius: 5.0)
    let rectangle = Rectangle(width: 3.0, height: 4.0)
    let triangle = Triangle(base: 3.0, height: 4.0, side3: 5.0)
    
    print("\(circle.describe()) with area \(circle.area())")
    print("\(rectangle.describe()) with area \(rectangle.area())")
    print("\(triangle.describe()) with area \(triangle.area())")
    print("")
    
    // Division
    print("Division:")
    let result1 = safeDivide(10, 2)
    switch result1 {
    case .success(let value):
        print("10 / 2 = \(value)")
    case .failure(let error):
        print("Error: \(error)")
    }
    
    let result2 = safeDivide(10, 0)
    switch result2 {
    case .success(let value):
        print("10 / 0 = \(value)")
    case .failure(let error):
        print("Error: \(error)")
    }
    print("")
    
    // Higher-order functions
    print("Higher-order Functions:")
    let numbers = [1, 2, 3, 4, 5]
    let doubled = numbers.map { $0 * 2 }
    print("Doubled: \(doubled)")
    
    let evens = numbers.filter { $0 % 2 == 0 }
    print("Even numbers: \(evens)")
    
    let sum = numbers.reduce(0, +)
    print("Sum: \(sum)")
    print("")
    
    // Array operations
    print("Array Operations:")
    print("Numbers: \(numbers)")
    print("First: \(numbers.first!)")
    print("Last: \(numbers.last!)")
    print("Count: \(numbers.count)")
    print("Reversed: \(numbers.reversed())")
    print("Sorted: \(numbers.sorted())")
    print("Contains 3: \(numbers.contains(3))")
    print("")
    
    // Dictionary operations
    print("Dictionary Operations:")
    var myDict = ["one": 1, "two": 2, "three": 3]
    print("Dict: \(myDict)")
    print("Value for 'two': \(myDict["two"]!)")
    print("Keys: \(myDict.keys)")
    print("Values: \(myDict.values)")
    
    myDict["four"] = 4
    print("Updated dict: \(myDict)")
    print("")
    
    // String operations
    print("String Operations:")
    let text = "Hello, World!"
    print("Uppercase: \(text.uppercased())")
    print("Lowercase: \(text.lowercased())")
    print("Length: \(text.count)")
    print("Has suffix '!': \(text.hasSuffix("!"))")
    print("Has prefix 'Hello': \(text.hasPrefix("Hello"))")
    print("Contains 'World': \(text.contains("World"))")
    print("")
    
    // Range operations
    print("Range Operations:")
    let range = 1...10
    print("Range sum: \(range.reduce(0, +))")
    print("Range map: \(range.map { $0 * 2 })")
    print("")
    
    // Optional handling
    print("Optional Handling:")
    var optionalString: String? = "Hello"
    print("Optional value: \(optionalString ?? "default")")
    
    optionalString = nil
    print("Optional value: \(optionalString ?? "default")")
    
    if let unwrapped = optionalString {
        print("Unwrapped: \(unwrapped)")
    } else {
        print("Optional is nil")
    }
    print("")
    
    // Enum operations
    print("Enum Operations:")
    let statuses: [Status] = [.pending, .processing, .completed, .cancelled]
    for status in statuses {
        print("Status: \(status)")
    }
    print("")
    
    // Closures
    print("Closures:")
    let add = { (a: Int, b: Int) -> Int in
        return a + b
    }
    print("add(2, 3) = \(add(2, 3))")
    
    let square = { $0 * $0 }
    print("square(5) = \(square(5))")
    print("")
    
    // Guard statement
    print("Guard Statement:")
    func processAge(_ age: Int) -> String {
        guard age >= 18 else {
            return "Minor"
        }
        return "Adult"
    }
    print("Age 25: \(processAge(25))")
    print("Age 15: \(processAge(15))")
    print("")
    
    // Tuples
    print("Tuple Operations:")
    let tuple = (1, "hello", 3.14)
    print("Tuple: \(tuple)")
    print("First element: \(tuple.0)")
    print("Second element: \(tuple.1)")
    print("Third element: \(tuple.2)")
    print("")
    
    // Sets
    print("Set Operations:")
    let set1: Set<Int> = [1, 2, 3, 4, 5]
    let set2: Set<Int> = [4, 5, 6, 7, 8]
    print("Set 1: \(set1)")
    print("Set 2: \(set2)")
    print("Union: \(set1.union(set2))")
    print("Intersection: \(set1.intersection(set2))")
    print("Difference: \(set1.subtracting(set2))")
    print("")
}

// Run main
main()
