/**
 * Comprehensive Java test file containing all entities and relationships.
 */

package com.example.geometry;

import java.util.*;
import java.util.function.*;

// Interface
interface Shape {
    double area();
    double perimeter();
    default void describe() {
        System.out.println("This is a shape");
    }
}

interface Drawable {
    void draw();
}

// Enum
enum Color {
    RED("Red", 0xFF0000),
    GREEN("Green", 0x00FF00),
    BLUE("Blue", 0x0000FF);
    
    private final String name;
    private final int rgb;
    
    Color(String name, int rgb) {
        this.name = name;
        this.rgb = rgb;
    }
    
    public String getName() {
        return name;
    }
    
    public int getRgb() {
        return rgb;
    }
    
    public String description() {
        return "This is " + name;
    }
}

// Enum with abstract method
enum Operation {
    PLUS { public double apply(double x, double y) { return x + y; } },
    MINUS { public double apply(double x, double y) { return x - y; } },
    TIMES { public double apply(double x, double y) { return x * y; } },
    DIVIDE { public double apply(double x, double y) { return x / y; } };
    
    public abstract double apply(double x, double y);
}

// Abstract class
abstract class AbstractShape implements Shape {
    protected String name;
    
    public AbstractShape(String name) {
        this.name = name;
    }
    
    @Override
    public void describe() {
        System.out.println("Abstract shape: " + name);
    }
}

// Class implementing multiple interfaces
class Circle extends AbstractShape implements Drawable {
    private double radius;
    
    public Circle(String name, double radius) {
        super(name);
        this.radius = radius;
    }
    
    @Override
    public double area() {
        return Math.PI * radius * radius;
    }
    
    @Override
    public double perimeter() {
        return 2 * Math.PI * radius;
    }
    
    @Override
    public void draw() {
        System.out.println("Drawing circle: " + name);
    }
}

// Generic class
class Container<T> {
    private List<T> items;
    
    public Container() {
        this.items = new ArrayList<>();
    }
    
    public void add(T item) {
        items.add(item);
    }
    
    public T get(int index) {
        return items.get(index);
    }
    
    public int size() {
        return items.size();
    }
}

// Generic class with bounds
class NumberContainer<T extends Number> {
    private List<T> numbers;
    
    public NumberContainer() {
        this.numbers = new ArrayList<>();
    }
    
    public void add(T number) {
        numbers.add(number);
    }
    
    public double sum() {
        double total = 0;
        for (T num : numbers) {
            total += num.doubleValue();
        }
        return total;
    }
}

// Generic method
class Utils {
    public static <T> void swap(T[] arr, int i, int j) {
        T temp = arr[i];
        arr[i] = arr[j];
        arr[j] = temp;
    }
    
    public static <T extends Comparable<T>> T max(T a, T b) {
        return a.compareTo(b) > 0 ? a : b;
    }
}

// Lambda and functional interfaces
class LambdaExample {
    public void demonstrateLambdas() {
        // Lambda expression
        Predicate<Integer> isEven = n -> n % 2 == 0;
        Function<String, Integer> stringLength = s -> s.length();
        Consumer<String> printer = s -> System.out.println(s);
        Supplier<Double> random = () -> Math.random();
        
        // Method reference
        Function<String, String> toUpper = String::toUpperCase;
        
        // Using lambdas
        System.out.println("Is 4 even? " + isEven.test(4));
        System.out.println("Length of 'hello': " + stringLength.apply("hello"));
        printer.accept("Hello from lambda");
        System.out.println("Random: " + random.get());
        System.out.println("Uppercase: " + toUpper.apply("hello"));
    }
}

// Streams
class StreamExample {
    public void demonstrateStreams() {
        List<Integer> numbers = Arrays.asList(1, 2, 3, 4, 5, 6, 7, 8, 9, 10);
        
        // Filter and map
        List<Integer> evenSquares = numbers.stream()
            .filter(n -> n % 2 == 0)
            .map(n -> n * n)
            .toList();
        
        // Reduce
        int sum = numbers.stream()
            .reduce(0, Integer::sum);
        
        // Collect
        String joined = numbers.stream()
            .map(Object::toString)
            .collect(Collectors.joining(", "));
        
        System.out.println("Even squares: " + evenSquares);
        System.out.println("Sum: " + sum);
        System.out.println("Joined: " + joined);
    }
}

// Annotations
@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.TYPE, ElementType.METHOD})
@interface MyAnnotation {
    String value() default "";
    int priority() default 0;
}

@MyAnnotation(value = "Person class", priority = 1)
class Person {
    private String name;
    private int age;
    private String email;
    
    public Person(String name, int age, String email) {
        this.name = name;
        this.age = age;
        this.email = email;
    }
    
    @MyAnnotation("Greet method")
    public void greet() {
        System.out.println("Hello, I'm " + name);
    }
    
    public static Person createDefault() {
        return new Person("Unknown", 0, "unknown@example.com");
    }
    
    // Getters
    public String getName() { return name; }
    public int getAge() { return age; }
    public String getEmail() { return email; }
}

// Exception handling
class CustomException extends Exception {
    public CustomException(String message) {
        super(message);
    }
}

class ExceptionExample {
    public void riskyOperation() throws CustomException {
        throw new CustomException("Something went wrong");
    }
    
    public void handleException() {
        try {
            riskyOperation();
        } catch (CustomException e) {
            System.out.println("Caught: " + e.getMessage());
        } finally {
            System.out.println("Finally block");
        }
    }
}

// Inner classes
class OuterClass {
    private int x = 10;
    
    // Non-static inner class
    class InnerClass {
        public void display() {
            System.out.println("Outer x: " + x);
        }
    }
    
    // Static nested class
    static class StaticNestedClass {
        public void display() {
            System.out.println("Static nested class");
        }
    }
    
    // Method-local inner class
    public void methodWithLocalClass() {
        class LocalClass {
            public void display() {
                System.out.println("Local class");
            }
        }
        LocalClass local = new LocalClass();
        local.display();
    }
}

// Functional interface
@FunctionalInterface
interface MathOperation {
    int operate(int a, int b);
}

// Thread and Runnable
class ThreadExample {
    public void demonstrateThreads() {
        // Using Runnable with lambda
        Runnable task = () -> {
            for (int i = 0; i < 5; i++) {
                System.out.println("Thread: " + i);
            }
        };
        
        Thread thread = new Thread(task);
        thread.start();
        
        try {
            thread.join();
        } catch (InterruptedException e) {
            e.printStackTrace();
        }
    }
}

// Record (Java 14+)
record Point(double x, double y) {
    public double distanceTo(Point other) {
        double dx = this.x - other.x;
        double dy = this.y - other.y;
        return Math.sqrt(dx * dx + dy * dy);
    }
}

// Main class
public class TestJava {
    public static void main(String[] args) {
        // Enum usage
        Color color = Color.RED;
        System.out.println(color.getName() + ": " + color.getRgb());
        
        // Operation enum
        double result = Operation.PLUS.apply(5, 3);
        System.out.println("5 + 3 = " + result);
        
        // Shape usage
        Circle circle = new Circle("MyCircle", 5.0);
        System.out.println("Area: " + circle.area());
        System.out.println("Perimeter: " + circle.perimeter());
        circle.draw();
        
        // Generic container
        Container<Integer> container = new Container<>();
        container.add(10);
        container.add(20);
        System.out.println("Container size: " + container.size());
        
        // Number container
        NumberContainer<Double> numbers = new NumberContainer<>();
        numbers.add(1.5);
        numbers.add(2.5);
        System.out.println("Sum: " + numbers.sum());
        
        // Utils
        Integer[] arr = {1, 2, 3};
        Utils.swap(arr, 0, 2);
        System.out.println("Max: " + Utils.max(5, 10));
        
        // Lambdas
        LambdaExample lambda = new LambdaExample();
        lambda.demonstrateLambdas();
        
        // Streams
        StreamExample stream = new StreamExample();
        stream.demonstrateStreams();
        
        // Person with annotation
        Person person = new Person("Alice", 25, "alice@example.com");
        person.greet();
        
        // Exception
        ExceptionExample ex = new ExceptionExample();
        ex.handleException();
        
        // Inner classes
        OuterClass outer = new OuterClass();
        OuterClass.InnerClass inner = outer.new InnerClass();
        inner.display();
        
        OuterClass.StaticNestedClass staticInner = new OuterClass.StaticNestedClass();
        staticInner.display();
        
        outer.methodWithLocalClass();
        
        // Threads
        ThreadExample thread = new ThreadExample();
        thread.demonstrateThreads();
        
        // Record
        Point p1 = new Point(0, 0);
        Point p2 = new Point(3, 4);
        System.out.println("Distance: " + p1.distanceTo(p2));
        
        // Functional interface
        MathOperation add = (a, b) -> a + b;
        MathOperation multiply = (a, b) -> a * b;
        System.out.println("Add: " + add.operate(5, 3));
        System.out.println("Multiply: " + multiply.operate(5, 3));
    }
}
