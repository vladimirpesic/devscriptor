/**
 * Comprehensive C++ test file containing all entities and relationships.
 */

#include <iostream>
#include <vector>
#include <string>
#include <functional>
#include <memory>
#include <algorithm>
#include <cmath>

// Namespace
namespace Geometry {
    struct Point {
        double x, y;
    };
    
    double distance(Point p1, Point p2) {
        return std::sqrt(std::pow(p2.x - p1.x, 2) + std::pow(p2.y - p1.y, 2));
    }
}

// Enum class
enum class Color {
    Red,
    Green,
    Blue
};

enum class Priority : int {
    Low = 1,
    Medium = 2,
    High = 3
};

// Struct
struct Person {
    std::string name;
    int age;
    std::string email;
    
    void print() const {
        std::cout << name << ", " << age << " years old\n";
    }
};

// Class
class Shape {
public:
    virtual double area() const = 0;
    virtual double perimeter() const = 0;
    virtual ~Shape() = default;
};

class Circle : public Shape {
private:
    double radius;
    
public:
    explicit Circle(double r) : radius(r) {}
    
    double area() const override {
        return M_PI * radius * radius;
    }
    
    double perimeter() const override {
        return 2 * M_PI * radius;
    }
    
    double getRadius() const { return radius; }
};

// Template class
template<typename T>
class Container {
private:
    std::vector<T> items;
    
public:
    void add(const T& item) {
        items.push_back(item);
    }
    
    const T& get(size_t index) const {
        return items.at(index);
    }
    
    size_t size() const {
        return items.size();
    }
};

// Template function
template<typename T>
T add(T a, T b) {
    return a + b;
}

// Template specialization
template<>
const char* add<const char*>(const char* a, const char* b) {
    static std::string result = std::string(a) + b;
    return result.c_str();
}

// Lambda functions
void demonstrate_lambdas() {
    auto add = [](int a, int b) -> int { return a + b; };
    auto multiply = [](int a, int b) { return a * b; };
    
    std::cout << "Lambda add: " << add(10, 20) << "\n";
    std::cout << "Lambda multiply: " << multiply(5, 6) << "\n";
    
    // Capture by value
    int factor = 2;
    auto scale = [factor](int x) { return x * factor; };
    std::cout << "Scaled: " << scale(5) << "\n";
    
    // Capture by reference
    int counter = 0;
    auto increment = [&counter]() { return ++counter; };
    std::cout << "Counter: " << increment() << "\n";
}

// Function pointer
using MathFunc = double(*)(double);
double apply(MathFunc func, double value) {
    return func(value);
}

// std::function
std::function<int(int, int)> get_operation(const std::string& op) {
    if (op == "add") {
        return [](int a, int b) { return a + b; };
    } else if (op == "multiply") {
        return [](int a, int b) { return a * b; };
    }
    return nullptr;
}

// Smart pointers
class Resource {
public:
    Resource() { std::cout << "Resource acquired\n"; }
    ~Resource() { std::cout << "Resource released\n"; }
    void doSomething() { std::cout << "Doing something\n"; }
};

void demonstrate_smart_pointers() {
    std::unique_ptr<Resource> ptr1 = std::make_unique<Resource>();
    ptr1->doSomething();
    
    std::shared_ptr<Resource> ptr2 = std::make_shared<Resource>();
    std::shared_ptr<Resource> ptr3 = ptr2;
    std::cout << "Shared count: " << ptr2.use_count() << "\n";
}

// Main function
int main() {
    // Namespace usage
    Geometry::Point p1{0, 0}, p2{3, 4};
    std::cout << "Distance: " << Geometry::distance(p1, p2) << "\n";
    
    // Class usage
    Circle circle(5.0);
    std::cout << "Circle area: " << circle.area() << "\n";
    std::cout << "Circle perimeter: " << circle.perimeter() << "\n";
    
    // Template class
    Container<int> intContainer;
    intContainer.add(10);
    intContainer.add(20);
    std::cout << "Container size: " << intContainer.size() << "\n";
    
    // Template function
    std::cout << "Add ints: " << add(5, 3) << "\n";
    std::cout << "Add doubles: " << add(5.5, 3.3) << "\n";
    
    // Struct
    Person person{"John Doe", 30, "john@example.com"};
    person.print();
    
    // Enum
    Color c = Color::Red;
    Priority p = Priority::High;
    
    // Lambdas
    demonstrate_lambdas();
    
    // Function pointer
    std::cout << "Apply sin: " << apply(std::sin, 0.5) << "\n";
    
    // Smart pointers
    demonstrate_smart_pointers();
    
    return 0;
}
