/**
 * Comprehensive Kotlin test file containing all entities and relationships.
 */

package com.example.geometry

import kotlinx.coroutines.*
import kotlin.properties.Delegates

// Interface
interface Shape {
    fun area(): Double
    fun perimeter(): Double
}

interface Drawable {
    fun draw()
}

// Sealed class
sealed class Result {
    data class Success(val value: Any) : Result()
    data class Error(val message: String) : Result()
    object Loading : Result()
}

// Enum class
enum class Color(val rgb: Int) {
    RED(0xFF0000),
    GREEN(0x00FF00),
    BLUE(0x0000FF);
    
    fun description(): String = "This is $name"
}

enum class Priority {
    LOW, MEDIUM, HIGH
}

// Data class
data class Point(val x: Double, val y: Double) {
    fun distanceTo(other: Point): Double {
        val dx = x - other.x
        val dy = y - other.y
        return Math.sqrt(dx * dx + dy * dy)
    }
}

// Object (singleton)
object Singleton {
    private var counter = 0
    
    fun increment() {
        counter++
    }
    
    fun count(): Int = counter
}

// Abstract class
abstract class AbstractShape(protected val name: String) : Shape {
    abstract override fun area(): Double
    abstract override fun perimeter(): Double
    
    open fun describe() {
        println("Shape: $name")
    }
}

// Class implementing multiple interfaces
class Circle(name: String, private val radius: Double) : AbstractShape(name), Drawable {
    override fun area(): Double = Math.PI * radius * radius
    override fun perimeter(): Double = 2 * Math.PI * radius
    override fun draw() {
        println("Drawing circle: $name")
    }
}

// Generic class
class Container<T> {
    private val items = mutableListOf<T>()
    
    fun add(item: T) {
        items.add(item)
    }
    
    fun get(index: Int): T = items[index]
    fun count(): Int = items.size
}

// Generic class with multiple type parameters
class Pair<A, B>(val first: A, val second: B)

// Generic class with upper bound
class NumberContainer<T : Number> {
    private val numbers = mutableListOf<T>()
    
    fun add(number: T) {
        numbers.add(number)
    }
    
    fun sum(): Double = numbers.sumOf { it.toDouble() }
}

// Generic function
fun <T> swap(list: MutableList<T>, i: Int, j: Int) {
    val temp = list[i]
    list[i] = list[j]
    list[j] = temp
}

fun <T : Comparable<T>> max(a: T, b: T): T = if (a > b) a else b

// Class with properties and backing fields
class Person(
    var name: String,
    var age: Int,
    email: String
) {
    var email: String = email
        set(value) {
            field = value.lowercase()
        }
    
    val isAdult: Boolean
        get() = age >= 18
    
    companion object {
        private var count = 0
        
        fun createDefault(): Person = Person("Unknown", 0, "unknown@example.com")
        fun count(): Int = count
    }
    
    init {
        count++
    }
    
    fun greet() {
        println("Hello, I'm $name")
    }
}

// Data class with copy
data class User(val id: Int, val name: String, val email: String)

// When expression (pattern matching)
fun matchPattern(value: Any?): String = when (value) {
    is Int -> "Integer: $value"
    is String -> "String: $value"
    is List<*> -> "List with ${value.size} items"
    is Boolean -> "Boolean: $value"
    null -> "Null"
    else -> "Unknown"
}

// Extension function
fun String.reverse(): String = this.reversed()

// Extension property
val String.isValidEmail: Boolean
    get() = contains("@") && contains(".")

// Higher-order functions and lambdas
fun operate(a: Int, b: Int, operation: (Int, Int) -> Int): Int = operation(a, b)

// Class with delegated property
class DelegatedProperties {
    var observable: Int by Delegates.observable(0) { property, oldValue, newValue ->
        println("$oldValue -> $newValue")
    }
    
    var vetoable: Int by Delegates.vetoable(0) { property, oldValue, newValue ->
        newValue >= 0
    }
}

// Coroutine
suspend fun fetchData(url: String): String {
    delay(100)
    return "Data from $url"
}

suspend fun processMultiple(urls: List<String>) = coroutineScope {
    urls.map { url ->
        async { fetchData(url) }
    }.awaitAll()
}

// Class with operator overloading
class ComplexNumber(val real: Double, val imaginary: Double) {
    operator fun plus(other: ComplexNumber): ComplexNumber =
        ComplexNumber(real + other.real, imaginary + other.imaginary)
    
    operator fun minus(other: ComplexNumber): ComplexNumber =
        ComplexNumber(real - other.real, imaginary - other.imaginary)
    
    operator fun times(other: ComplexNumber): ComplexNumber =
        ComplexNumber(
            real * other.real - imaginary * other.imaginary,
            real * other.imaginary + imaginary * other.real
        )
    
    override fun toString(): String = "$real + ${imaginary}i"
}

// Inline class
@JvmInline
value class UserId(val value: Int)

// Tail recursion
tailrec fun factorial(n: Int, accumulator: Int = 1): Int =
    if (n <= 1) accumulator else factorial(n - 1, accumulator * n)

// Null safety
fun findUser(id: Int): Person? {
    return if (id == 1) Person("John", 30, "john@example.com") else null
}

fun getUserNameLength(user: Person?): Int = user?.name?.length ?: 0

// Elvis operator
fun getFullName(firstName: String?, lastName: String?): String {
    return "${firstName ?: "Unknown"} ${lastName ?: "Unknown"}"
}

// Range and progression
fun ranges() {
    for (i in 1..10) {
        println(i)
    }
    
    for (i in 10 downTo 1 step 2) {
        println(i)
    }
}

// Collection operations
fun collectionOperations() {
    val numbers = listOf(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)
    
    val evenSquares = numbers
        .filter { it % 2 == 0 }
        .map { it * it }
    
    val sum = numbers.reduce { acc, num -> acc + num }
    
    val grouped = numbers.groupBy { if (it % 2 == 0) "even" else "odd" }
    
    val joined = numbers.joinToString(", ")
    
    println("Even squares: $evenSquares")
    println("Sum: $sum")
    println("Grouped: $grouped")
    println("Joined: $joined")
}

// Function with default arguments
fun greet(name: String, greeting: String = "Hello") {
    println("$greeting, $name!")
}

// Function with varargs
fun sum(vararg numbers: Int): Int = numbers.sum()

// Destructuring
data class Rectangle(val width: Double, val height: Double)

fun destructure(rectangle: Rectangle) {
    val (width, height) = rectangle
    println("Width: $width, Height: $height")
}

// Lazy initialization
class LazyInitialization {
    private val heavyObject by lazy {
        println("Initializing heavy object")
        "Heavy object created"
    }
    
    fun getObject(): String = heavyObject
}

// Late initialization
class LateInitialization {
    private lateinit var value: String
    
    fun init(value: String) {
        this.value = value
    }
    
    fun getValue(): String = value
}

// Annotation class
@Target(AnnotationTarget.CLASS, AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class CustomAnnotation(
    val description: String = "",
    val priority: Int = 0
)

@CustomAnnotation(description = "Person class", priority = 1)
class AnnotatedPerson(val name: String) {
    @CustomAnnotation("Greet method")
    fun greet() {
        println("Hello, $name")
    }
}

// Sealed interface
sealed interface State {
    data class Ready(val data: String) : State
    data class Error(val message: String) : State
    object Loading : State
}

// Exception handling
class CustomException(message: String) : Exception(message)

fun riskyOperation() {
    throw CustomException("Something went wrong")
}

fun handleException() {
    try {
        riskyOperation()
    } catch (e: CustomException) {
        println("Caught: ${e.message}")
    } finally {
        println("Finally block")
    }
}

// Main function
fun main() {
    // Namespace/package usage
    val p1 = Point(0.0, 0.0)
    val p2 = Point(3.0, 4.0)
    println("Distance: ${p1.distanceTo(p2)}")
    
    // Enum usage
    val color = Color.RED
    println("Color: ${color.description()}")
    
    // Singleton
    Singleton.increment()
    println("Singleton count: ${Singleton.count()}")
    
    // Shape usage
    val circle = Circle("MyCircle", 5.0)
    println("Area: ${circle.area()}")
    println("Perimeter: ${circle.perimeter()}")
    circle.draw()
    
    // Generic container
    val container = Container<Int>()
    container.add(10)
    container.add(20)
    println("Container count: ${container.count()}")
    
    // Pair
    val pair = Pair("key", 42)
    println("Pair: ${pair.first} -> ${pair.second}")
    
    // Number container
    val numbers = NumberContainer<Double>()
    numbers.add(1.5)
    numbers.add(2.5)
    println("Sum: ${numbers.sum()}")
    
    // Person
    val person = Person("Alice", 25, "alice@example.com")
    person.greet()
    println("Is adult: ${person.isAdult}")
    
    // Data class
    val user = User(1, "Bob", "bob@example.com")
    val updatedUser = user.copy(name = "Robert")
    println("User: $user")
    println("Updated: $updatedUser")
    
    // When expression
    println(matchPattern(42))
    println(matchPattern("hello"))
    println(matchPattern(listOf(1, 2, 3)))
    
    // Extension function
    println("hello".reverse())
    
    // Higher-order function
    val sum = operate(5, 3) { a, b -> a + b }
    println("Sum: $sum")
    
    // Complex numbers
    val c1 = ComplexNumber(1.0, 2.0)
    val c2 = ComplexNumber(3.0, 4.0)
    println("Add: ${c1 + c2}")
    println("Subtract: ${c1 - c2}")
    println("Multiply: ${c1 * c2}")
    
    // Factorial
    println("Factorial 5: ${factorial(5)}")
    
    // Null safety
    val user2 = findUser(1)
    println("User name length: ${getUserNameLength(user2)}")
    
    // Default arguments
    greet("World")
    greet("Alice", "Hi")
    
    // Varargs
    println("Sum: ${sum(1, 2, 3, 4, 5)}")
    
    // Destructuring
    destructure(Rectangle(5.0, 10.0))
    
    // Collections
    collectionOperations()
    
    // Ranges
    ranges()
    
    // Exception
    handleException()
}
