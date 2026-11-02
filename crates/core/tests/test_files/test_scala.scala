// Comprehensive Scala test file

import scala.collection.mutable
import scala.util.{Try, Success, Failure}

// Case class for immutable data
case class User(
  id: Int,
  username: String,
  email: String,
  age: Int,
  isActive: Boolean
)

// Sealed trait for enum-like behavior
sealed trait Status
case object Pending extends Status
case object Processing extends Status
case object Completed extends Status
case object Cancelled extends Status

// Companion object
object User {
  def apply(username: String, email: String, age: Int): User = {
    val id = hashCode.abs
    User(id, username, email, age, isActive = true)
  }
}

// Trait for polymorphic behavior
trait Describable {
  def describe: String
}

class Shape extends Describable {
  def describe: String = "A generic shape"
}

case class Circle(radius: Double) extends Describable {
  def area: Double = Math.PI * radius * radius
  def describe: String = s"Circle with radius $radius"
}

case class Rectangle(width: Double, height: Double) extends Describable {
  def area: Double = width * height
  def describe: String = s"Rectangle ${width}x$height"
}

case class Triangle(base: Double, height: Double, side3: Double) extends Describable {
  def area: Double = 0.5 * base * height
  def describe: String = s"Triangle $base-$height-$side3"
}

// Generic class
class Container[A](value: A) {
  def get: A = value
  def map[B](f: A => B): Container[B] = Container(f(value))
  def flatMap[B](f: A => Container[B]): Container[B] = f(value)
}

// Monad for Option-like behavior
case class Maybe[A](value: Option[A]) {
  def map[B](f: A => B): Maybe[B] = Maybe(value.map(f))
  def flatMap[B](f: A => Maybe[B]): Maybe[B] = Maybe(value.flatMap(a => f(a).value))
  def getOrElse(default: A): A = value.getOrElse(default)
}

// Singleton object with utility methods
object Utils {
  def isAdult(age: Int): Boolean = age >= 18
  
  def findUser(users: List[User], id: Int): Option[User] = {
    users.find(_.id == id)
  }
  
  def filterActiveUsers(users: List[User]): List[User] = {
    users.filter(_.isActive)
  }
  
  def mapUserNames(users: List[User]): List[String] = {
    users.map(_.username)
  }
  
  def sumAges(users: List[User]): Int = {
    users.map(_.age).sum
  }
  
  def averageAge(users: List[User]): Double = {
    if (users.isEmpty) 0.0
    else sumAges(users).toDouble / users.length
  }
}

// Trait with default implementation
trait Animal {
  def sound: String
  def speak: String = s"The animal says: $sound"
}

class Dog extends Animal {
  def sound: String = "Woof!"
  
  override def speak: String = s"The dog barks: $sound"
}

class Cat extends Animal {
  def sound: String = "Meow!"
}

// Pattern matching example
object PatternMatching {
  def matchStatus(status: Status): String = status match {
    case Pending => "Order is pending"
    case Processing => "Order is being processed"
    case Completed => "Order completed successfully"
    case Cancelled => "Order was cancelled"
  }
  
  def matchUser(user: User): String = user match {
    case User(_, username, _, age, true) if age >= 18 => s"Active adult user: $username"
    case User(_, username, _, age, true) => s"Active minor user: $username"
    case User(_, username, _, _, false) => s"Inactive user: $username"
  }
  
  def describeShape(shape: Describable): String = shape match {
    case c: Circle => s"${c.describe} with area ${c.area}"
    case r: Rectangle => s"${r.describe} with area ${r.area}"
    case t: Triangle => s"${t.describe} with area ${t.area}"
    case s => s.describe
  }
}

// Recursion examples
object Recursion {
  def factorial(n: Int): Int = {
    if (n <= 1) 1
    else n * factorial(n - 1)
  }
  
  def fibonacci(n: Int): Int = {
    if (n <= 1) n
    else fibonacci(n - 1) + fibonacci(n - 2)
  }
  
  def sumList(list: List[Int]): Int = {
    list.foldLeft(0)(_ + _)
  }
  
  def reverseList(list: List[Any]): List[Any] = {
    list.foldRight(List.empty[Any])((elem, acc) => acc :+ elem)
  }
}

// Higher-order functions
object HigherOrder {
  def map[A, B](list: List[A])(f: A => B): List[B] = {
    list.map(f)
  }
  
  def filter[A](list: List[A])(predicate: A => Boolean): List[A] = {
    list.filter(predicate)
  }
  
  def reduce[A](list: List[A])(op: (A, A) => A): A = {
    list.reduce(op)
  }
}

// Error handling
object ErrorHandling {
  def divide(numerator: Double, denominator: Double): Try[Double] = {
    Try(numerator / denominator)
  }
  
  def safeDivide(numerator: Double, denominator: Double): Either[String, Double] = {
    if (denominator == 0) Left("Division by zero")
    else Right(numerator / denominator)
  }
}

// Immutable collections
object Collections {
  val list = List(1, 2, 3, 4, 5)
  val set = Set(1, 2, 3, 4, 5)
  val map = Map(
    "one" -> 1,
    "two" -> 2,
    "three" -> 3
  )
  
  def transformList(list: List[Int]): List[Int] = {
    list.filter(_ % 2 == 0).map(_ * 2)
  }
  
  def groupByAge(users: List[User]): Map[Int, List[User]] = {
    users.groupBy(_.age)
  }
}

// Lazy evaluation
object LazyEvaluation {
  lazy val expensiveValue: Int = {
    println("Computing expensive value...")
    Thread.sleep(1000)
    42
  }
}

// String interpolation and operations
object Strings {
  def greet(name: String): String = {
    s"Hello, $name!"
  }
  
  def formatUserInfo(user: User): String = {
    s"""User Information:
       |ID: ${user.id}
       |Name: ${user.username}
       |Email: ${user.email}
       |Age: ${user.age}
       |Active: ${user.isActive}""".stripMargin
  }
}

// Main object
object Main extends App {
  println("=== Scala Test Program ===\n")
  
  // Create sample users
  val users = List(
    User(1, "Alice", "alice@example.com", 25, true),
    User(2, "Bob", "bob@example.com", 30, true),
    User(3, "Charlie", "charlie@example.com", 17, false)
  )
  
  // Print user information
  println("Users:")
  users.foreach { user =>
    println(Strings.formatUserInfo(user))
  }
  
  // Filter active users
  println("\nActive Users:")
  Utils.filterActiveUsers(users).foreach(user => println(user.username))
  
  // Adult users
  println("\nAdult Users:")
  users.filter(u => Utils.isAdult(u.age)).foreach(user => println(user.username))
  
  // Find specific user
  println("\nFinding user with ID 2:")
  Utils.findUser(users, 2) match {
    case Some(user) => println(s"Found: ${user.username}")
    case None => println("User not found")
  }
  
  // Calculations
  println("\nCalculations:")
  println(s"Total ages: ${Utils.sumAges(users)}")
  println(s"Average age: ${Utils.averageAge(users)}")
  
  // Factorial and Fibonacci
  println(s"\nFactorial 5: ${Recursion.factorial(5)}")
  println(s"Fibonacci 10: ${Recursion.fibonacci(10)}")
  
  // Shapes
  println("\nShapes:")
  val circle = Circle(5.0)
  val rectangle = Rectangle(3.0, 4.0)
  val triangle = Triangle(3.0, 4.0, 5.0)
  
  println(PatternMatching.describeShape(circle))
  println(PatternMatching.describeShape(rectangle))
  println(PatternMatching.describeShape(triangle))
  
  // Status matching
  println("\nStatus:")
  println(PatternMatching.matchStatus(Pending))
  println(PatternMatching.matchStatus(Completed))
  
  // User pattern matching
  println("\nUser Pattern Matching:")
  users.foreach(user => println(PatternMatching.matchUser(user)))
  
  // Error handling
  println("\nDivision:")
  println(s"10 / 2: ${ErrorHandling.divide(10, 2)}")
  println(s"10 / 0: ${ErrorHandling.divide(10, 0)}")
  
  println(s"\nSafe divide 10 / 2: ${ErrorHandling.safeDivide(10, 2)}")
  println(s"Safe divide 10 / 0: ${ErrorHandling.safeDivide(10, 0)}")
  
  // Collections
  println("\nCollections:")
  println(s"List: ${Collections.list}")
  println(s"Set: ${Collections.set}")
  println(s"Map: ${Collections.map}")
  println(s"Transformed list: ${Collections.transformList(Collections.list)}")
  
  // Higher-order functions
  println("\nHigher-order Functions:")
  println(s"Squared: ${HigherOrder.map(List(1, 2, 3, 4, 5))(_ * 2)}")
  println(s"Even numbers: ${HigherOrder.filter(List(1, 2, 3, 4, 5))(_ % 2 == 0)}")
  println(s"Sum: ${HigherOrder.reduce(List(1, 2, 3, 4, 5))(_ + _)}")
  
  // Generic Container
  println("\nGeneric Container:")
  val container = Container(10)
  println(s"Original: ${container.get}")
  println(s"Mapped: ${container.map(_ * 2).get}")
  
  // Maybe monad
  println("\nMaybe Monad:")
  val maybe1 = Maybe(Some(10))
  val maybe2 = Maybe(None)
  println(s"Maybe 1: ${maybe1.getOrElse(0)}")
  println(s"Maybe 2: ${maybe2.getOrElse(0)}")
  
  // Animals
  println("\nAnimals:")
  val dog = new Dog
  val cat = new Cat
  println(dog.speak)
  println(cat.speak)
  
  // User companion object
  println("\nCompanion Object:")
  val newUser = User("David", "david@example.com", 28)
  println(s"New user: $newUser")
  
  // Group by age
  println("\nGroup users by age:")
  val grouped = Collections.groupByAge(users)
  grouped.foreach { case (age, userList) =>
    println(s"Age $age: ${userList.map(_.username).mkString(", ")}")
  }
}
