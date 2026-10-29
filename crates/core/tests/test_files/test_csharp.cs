/**
 * Comprehensive C# test file containing all entities and relationships.
 */

using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using System.Linq;

// Namespace
namespace Geometry
{
    public struct Point
    {
        public double X { get; set; }
        public double Y { get; set; }
        
        public Point(double x, double y)
        {
            X = x;
            Y = y;
        }
    }
}

// Enum
public enum Color
{
    Red,
    Green,
    Blue
}

// Enum with underlying type
public enum Priority : int
{
    Low = 1,
    Medium = 2,
    High = 3
}

// Interface
public interface IShape
{
    double Area();
    double Perimeter();
}

public interface IDrawable
{
    void Draw();
}

// Abstract class
public abstract class Shape : IShape
{
    public string Name { get; protected set; }
    
    protected Shape(string name)
    {
        Name = name;
    }
    
    public abstract double Area();
    public abstract double Perimeter();
    
    public virtual void Describe()
    {
        Console.WriteLine($"Shape: {Name}");
    }
}

// Class implementing interface and inheriting
public class Circle : Shape, IDrawable
{
    public double Radius { get; private set; }
    
    public Circle(string name, double radius) : base(name)
    {
        Radius = radius;
    }
    
    public override double Area()
    {
        return Math.PI * Radius * Radius;
    }
    
    public override double Perimeter()
    {
        return 2 * Math.PI * Radius;
    }
    
    public void Draw()
    {
        Console.WriteLine($"Drawing circle {Name} with radius {Radius}");
    }
}

// Generic class
public class Container<T>
{
    private List<T> items;
    
    public Container()
    {
        items = new List<T>();
    }
    
    public void Add(T item)
    {
        items.Add(item);
    }
    
    public T Get(int index)
    {
        return items[index];
    }
    
    public int Count => items.Count;
}

// Generic class with multiple type parameters
public class Dictionary<TKey, TValue>
{
    private System.Collections.Generic.Dictionary<TKey, TValue> dict;
    
    public Dictionary()
    {
        dict = new System.Collections.Generic.Dictionary<TKey, TValue>();
    }
    
    public void Add(TKey key, TValue value)
    {
        dict[key] = value;
    }
    
    public bool TryGetValue(TKey key, out TValue value)
    {
        return dict.TryGetValue(key, out value);
    }
}

// Class with properties
public class Person
{
    public string Name { get; set; }
    public int Age { get; private set; }
    public string Email { get; set; }
    
    public Person(string name, int age, string email)
    {
        Name = name;
        Age = age;
        Email = email;
    }
    
    // Method
    public void Greet()
    {
        Console.WriteLine($"Hello, I'm {Name}");
    }
    
    // Static method
    public static Person CreateDefault()
    {
        return new Person("Unknown", 0, "unknown@example.com");
    }
}

// Async method
public class DataLoader
{
    public async Task<string> FetchDataAsync(string url)
    {
        await Task.Delay(100);
        return $"Data from {url}";
    }
    
    public async Task ProcessMultipleAsync(List<string> urls)
    {
        var tasks = urls.Select(u => FetchDataAsync(u));
        await Task.WhenAll(tasks);
    }
}

// Pattern matching
public class PatternMatcher
{
    public string Match(object obj)
    {
        return obj switch
        {
            int i => $"Integer: {i}",
            string s => $"String: {s}",
            bool b => $"Boolean: {b}",
            null => "Null",
            _ => "Unknown"
        };
    }
    
    public string MatchWithWhen(object obj)
    {
        return obj switch
        {
            int i when i > 0 => $"Positive integer: {i}",
            int i when i < 0 => $"Negative integer: {i}",
            int _ => "Zero",
            _ => "Not an integer"
        };
    }
}

// Attribute (annotation)
[AttributeUsage(AttributeTargets.Class | AttributeTargets.Method)]
public class CustomAttribute : Attribute
{
    public string Description { get; }
    
    public CustomAttribute(string description)
    {
        Description = description;
    }
}

[Custom("This is a custom class")]
public class AnnotatedClass
{
    [Custom("This is a custom method")]
    public void AnnotatedMethod()
    {
        Console.WriteLine("Annotated method called");
    }
}

// Delegate
public delegate int MathOperation(int a, int b);

public class DelegateExample
{
    public int Add(int a, int b) => a + b;
    public int Multiply(int a, int b) => a * b;
    
    public int Execute(MathOperation operation, int a, int b)
    {
        return operation(a, b);
    }
}

// Event
public class EventPublisher
{
    public event EventHandler<EventArgs> SomethingHappened;
    
    public void DoSomething()
    {
        Console.WriteLine("Doing something...");
        OnSomethingHappened();
    }
    
    protected virtual void OnSomethingHappened()
    {
        SomethingHappened?.Invoke(this, EventArgs.Empty);
    }
}

public class EventSubscriber
{
    public void Subscribe(EventPublisher publisher)
    {
        publisher.SomethingHappened += HandleEvent;
    }
    
    private void HandleEvent(object sender, EventArgs e)
    {
        Console.WriteLine("Event handled!");
    }
}

// Record (C# 9.0)
public record PersonRecord(string Name, int Age, string Email);

// Partial class
public partial class PartialClass
{
    public void Method1()
    {
        Console.WriteLine("Method 1");
    }
}

public partial class PartialClass
{
    public void Method2()
    {
        Console.WriteLine("Method 2");
    }
}

// Extension method
public static class StringExtensions
{
    public static string Reverse(this string str)
    {
        char[] arr = str.ToCharArray();
        Array.Reverse(arr);
        return new string(arr);
    }
}

// Indexer
public class IndexerExample
{
    private int[] items = new int[100];
    
    public int this[int index]
    {
        get => items[index];
        set => items[index] = value;
    }
}

// Main program
public class Program
{
    public static void Main(string[] args)
    {
        // Namespace usage
        Geometry.Point p = new Geometry.Point(3, 4);
        
        // Enum usage
        Color color = Color.Red;
        Priority priority = Priority.High;
        
        // Interface and class usage
        IShape shape = new Circle("MyCircle", 5.0);
        Console.WriteLine($"Area: {shape.Area()}");
        Console.WriteLine($"Perimeter: {shape.Perimeter()}");
        shape.Describe();
        
        // Generic container
        Container<int> container = new Container<int>();
        container.Add(10);
        container.Add(20);
        Console.WriteLine($"Count: {container.Count}");
        
        // Person class
        Person person = new Person("Alice", 25, "alice@example.com");
        person.Greet();
        
        Person defaultPerson = Person.CreateDefault();
        
        // Pattern matching
        PatternMatcher matcher = new PatternMatcher();
        Console.WriteLine(matcher.Match(42));
        Console.WriteLine(matcher.Match("hello"));
        
        // Delegate
        DelegateExample del = new DelegateExample();
        MathOperation operation = del.Add;
        Console.WriteLine($"Add: {del.Execute(operation, 10, 5)}");
        
        // Event
        EventPublisher publisher = new EventPublisher();
        EventSubscriber subscriber = new EventSubscriber();
        subscriber.Subscribe(publisher);
        publisher.DoSomething();
        
        // Record
        PersonRecord record = new PersonRecord("Bob", 30, "bob@example.com");
        
        // Partial class
        PartialClass partial = new PartialClass();
        partial.Method1();
        partial.Method2();
        
        // Extension method
        string reversed = "hello".Reverse();
        Console.WriteLine($"Reversed: {reversed}");
        
        // Indexer
        IndexerExample indexer = new IndexerExample();
        indexer[0] = 100;
        Console.WriteLine($"Indexer[0]: {indexer[0]}");
    }
}
