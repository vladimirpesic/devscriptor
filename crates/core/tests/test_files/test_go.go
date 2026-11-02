/**
 * Comprehensive Go test file containing all entities and relationships.
 */

package main

import (
	"fmt"
	"math"
)

// Interface
type Shape interface {
	Area() float64
	Perimeter() float64
}

// Struct
type Circle struct {
	Name   string
	Radius float64
}

// Method on struct
func (c *Circle) Area() float64 {
	return math.Pi * c.Radius * c.Radius
}

func (c *Circle) Perimeter() float64 {
	return 2 * math.Pi * c.Radius
}

// Method with value receiver
func (c Circle) String() string {
	return fmt.Sprintf("Circle %s (r=%.2f)", c.Name, c.Radius)
}

// Interface with multiple methods
type Writer interface {
	Write(data []byte) (int, error)
	Flush() error
}

// Struct implementing interface
type File struct {
	filename string
	data     []byte
}

func (f *File) Write(data []byte) (int, error) {
	f.data = append(f.data, data...)
	return len(data), nil
}

func (f *File) Flush() error {
	fmt.Printf("Flushing %s\n", f.filename)
	f.data = nil
	return nil
}

// Embedded struct
type Animal struct {
	Name string
}

func (a *Animal) Speak() string {
	return fmt.Sprintf("%s makes a sound", a.Name)
}

type Dog struct {
	Animal
	Breed string
}

func (d *Dog) Bark() string {
	return fmt.Sprintf("%s barks", d.Name)
}

// Function
func add(a, b int) int {
	return a + b
}

func multiply(a, b int) int {
	return a * b
}

// Function with multiple return values
func divide(a, b int) (int, error) {
	if b == 0 {
		return 0, fmt.Errorf("division by zero")
	}
	return a / b, nil
}

// Variadic function
func sum(numbers ...int) int {
	total := 0
	for _, num := range numbers {
		total += num
	}
	return total
}

// Higher-order function
func applyOperation(a, b int, op func(int, int) int) int {
	return op(a, b)
}

// Closure
func makeAdder(x int) func(int) int {
	return func(y int) int {
		return x + y
	}
}

// Method as function
func processShape(s Shape) {
	fmt.Printf("Area: %.2f, Perimeter: %.2f\n", s.Area(), s.Perimeter())
}

// Anonymous function
func demonstrateAnonymous() {
	add := func(a, b int) int { return a + b }
	fmt.Println("Anonymous:", add(5, 3))
}

// Goroutine (indicated by keyword)
func asyncTask(result chan int) {
	result <- 42
}

// Defer
func demonstrateDefer() {
	defer fmt.Println("Deferred 1")
	defer fmt.Println("Deferred 2")
	fmt.Println("Main")
}

// Type alias
type ID = string
type Age = int

// Custom type
type Person struct {
	Name  string
	Age   int
	Email string
}

// Constructor pattern
func NewPerson(name string, age int, email string) *Person {
	return &Person{
		Name:  name,
		Age:   age,
		Email: email,
	}
}

// Method on custom type
func (p *Person) Greet() string {
	return fmt.Sprintf("Hello, I'm %s", p.Name)
}

// Channel
func processChannels() {
	ch := make(chan int, 10)
	ch <- 1
	ch <- 2
	close(ch)

	for v := range ch {
		fmt.Println("Channel:", v)
	}
}

// Main function
func main() {
	// Struct and method usage
	circle := &Circle{Name: "MyCircle", Radius: 5.0}
	fmt.Println(circle)
	processShape(circle)

	// Interface usage
	file := &File{filename: "test.txt"}
	file.Write([]byte("Hello"))
	file.Flush()

	// Embedded struct
	dog := &Dog{
		Animal: Animal{Name: "Buddy"},
		Breed:  "Golden Retriever",
	}
	fmt.Println(dog.Speak())
	fmt.Println(dog.Bark())

	// Function calls
	fmt.Println("Add:", add(10, 20))
	fmt.Println("Multiply:", multiply(5, 6))

	// Multiple return values
	result, err := divide(10, 2)
	if err != nil {
		fmt.Println("Error:", err)
	} else {
		fmt.Println("Divide:", result)
	}

	// Variadic function
	fmt.Println("Sum:", sum(1, 2, 3, 4, 5))

	// Higher-order function
	result2 := applyOperation(10, 5, multiply)
	fmt.Println("Applied:", result2)

	// Closure
	add10 := makeAdder(10)
	fmt.Println("Add10:", add10(5))

	// Custom type
	person := NewPerson("Alice", 25, "alice@example.com")
	fmt.Println(person.Greet())

	// Anonymous function
	demonstrateAnonymous()

	// Defer
	demonstrateDefer()

	// Channels
	processChannels()
}
