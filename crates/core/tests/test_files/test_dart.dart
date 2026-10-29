// Comprehensive Dart test file

class User {
  final int id;
  final String username;
  final String email;
  final int age;
  final bool isActive;

  User({
    required this.id,
    required this.username,
    required this.email,
    required this.age,
    this.isActive = true,
  });

  @override
  String toString() {
    return 'User(id: $id, username: $username, email: $email, age: $age, isActive: $isActive)';
  }
}

// Enum for status
enum Status { pending, processing, completed, cancelled }

// Abstract class for shapes
abstract class Shape {
  double area();
  String describe();
}

class Circle extends Shape {
  final double radius;

  Circle(this.radius);

  @override
  double area() => 3.14159 * radius * radius;

  @override
  String describe() => 'Circle with radius $radius';
}

class Rectangle extends Shape {
  final double width;
  final double height;

  Rectangle(this.width, this.height);

  @override
  double area() => width * height;

  @override
  String describe() => 'Rectangle ${width}x$height';
}

class Triangle extends Shape {
  final double base;
  final double height;
  final double side3;

  Triangle(this.base, this.height, this.side3);

  @override
  double area() => 0.5 * base * height;

  @override
  String describe() => 'Triangle $base-$height-$side3';
}

// Generic class
class Container<T> {
  final T value;

  Container(this.value);

  T get() => value;

  Container<R> map<R>(R Function(T) fn) => Container(fn(value));

  Container<R> flatMap<R>(Container<R> Function(T) fn) => fn(value);
}

// Utility functions
bool isAdult(int age) => age >= 18;

User? findUser(List<User> users, int id) {
  for (var user in users) {
    if (user.id == id) return user;
  }
  return null;
}

List<User> filterActiveUsers(List<User> users) {
  return users.where((user) => user.isActive).toList();
}

List<String> mapUserNames(List<User> users) {
  return users.map((user) => user.username).toList();
}

int sumAges(List<User> users) {
  return users.fold(0, (sum, user) => sum + user.age);
}

double averageAge(List<User> users) {
  if (users.isEmpty) return 0.0;
  return sumAges(users) / users.length;
}

// Recursion
int factorial(int n) {
  if (n <= 1) return 1;
  return n * factorial(n - 1);
}

int fibonacci(int n) {
  if (n <= 1) return n;
  return fibonacci(n - 1) + fibonacci(n - 2);
}

// Higher-order functions
List<R> mapList<T, R>(List<T> list, R Function(T) fn) {
  return list.map(fn).toList();
}

List<T> filterList<T>(List<T> list, bool Function(T) predicate) {
  return list.where(predicate).toList();
}

T reduceList<T>(List<T> list, T Function(T, T) fn, T initial) {
  return list.fold(initial, fn);
}

// Error handling
Result<double> safeDivide(double numerator, double denominator) {
  if (denominator == 0) {
    return Result.error('Division by zero');
  }
  return Result.success(numerator / denominator);
}

class Result<T> {
  final T? value;
  final String? error;
  final bool isSuccess;

  Result.success(this.value) : error = null, isSuccess = true;

  Result.error(this.error) : value = null, isSuccess = false;

  R fold<R>(R Function(T) onSuccess, R Function(String) onError) {
    if (isSuccess) return onSuccess(value as T);
    return onError(error!);
  }
}

// Extension methods
extension StringExtension on String {
  String capitalize() {
    if (isEmpty) return this;
    return this[0].toUpperCase() + substring(1);
  }

  String reverse() {
    return split('').reversed.join();
  }
}

extension ListExtension<T> on List<T> {
  T? firstWhereOrNull(bool Function(T) predicate) {
    for (var item in this) {
      if (predicate(item)) return item;
    }
    return null;
  }
}

// Async example
Future<String> fetchData() async {
  await Future.delayed(Duration(milliseconds: 100));
  return 'Data loaded';
}

// Stream example
Stream<int> countStream(int max) async* {
  for (int i = 1; i <= max; i++) {
    await Future.delayed(Duration(milliseconds: 10));
    yield i;
  }
}

// Mixin
mixin class Logger {
  void log(String message) {
    print('[LOG] $message');
  }
}

class UserService with Logger {
  void createUser(String name) {
    log('Creating user: $name');
  }
}

// Main function
void main() {
  print('=== Dart Test Program ===\n');

  // Create sample users
  final users = [
    User(id: 1, username: 'Alice', email: 'alice@example.com', age: 25),
    User(id: 2, username: 'Bob', email: 'bob@example.com', age: 30),
    User(
      id: 3,
      username: 'Charlie',
      email: 'charlie@example.com',
      age: 17,
      isActive: false,
    ),
  ];

  // Print user information
  print('Users:');
  for (var user in users) {
    print(user);
    print('User Information:');
    print('  ID: ${user.id}');
    print('  Name: ${user.username}');
    print('  Email: ${user.email}');
    print('  Age: ${user.age}');
    print('  Active: ${user.isActive}');
    print('');
  }

  // Filter active users
  print('Active Users:');
  final activeUsers = filterActiveUsers(users);
  for (var user in activeUsers) {
    print(user.username);
  }
  print('');

  // Adult users
  print('Adult Users:');
  for (var user in users) {
    if (isAdult(user.age)) {
      print(user.username);
    }
  }
  print('');

  // Find specific user
  print('Finding user with ID 2:');
  final foundUser = findUser(users, 2);
  if (foundUser != null) {
    print('Found: ${foundUser.username}');
  } else {
    print('User not found');
  }
  print('');

  // Calculations
  print('Calculations:');
  print('Total ages: ${sumAges(users)}');
  print('Average age: ${averageAge(users)}');
  print('');

  // Factorial and Fibonacci
  print('Factorial 5: ${factorial(5)}');
  print('Fibonacci 10: ${fibonacci(10)}');
  print('');

  // Shapes
  print('Shapes:');
  final circle = Circle(5.0);
  final rectangle = Rectangle(3.0, 4.0);
  final triangle = Triangle(3.0, 4.0, 5.0);

  print('${circle.describe()} with area ${circle.area()}');
  print('${rectangle.describe()} with area ${rectangle.area()}');
  print('${triangle.describe()} with area ${triangle.area()}');
  print('');

  // Division with error handling
  print('Division:');
  final result1 = safeDivide(10, 2);
  print(
    '10 / 2: ${result1.fold((value) => value.toString(), (error) => 'Error: $error')}',
  );

  final result2 = safeDivide(10, 0);
  print(
    '10 / 0: ${result2.fold((value) => value.toString(), (error) => 'Error: $error')}',
  );
  print('');

  // Higher-order functions
  print('Higher-order Functions:');
  final numbers = [1, 2, 3, 4, 5];
  final doubled = mapList(numbers, (x) => x * 2);
  print('Doubled: $doubled');

  final evens = filterList(numbers, (x) => x % 2 == 0);
  print('Even numbers: $evens');

  final sum = reduceList(numbers, (a, b) => a + b, 0);
  print('Sum: $sum');
  print('');

  // Generic Container
  print('Generic Container:');
  final container = Container(10);
  print('Original: ${container.get()}');
  print('Mapped: ${container.map((x) => x * 2).get()}');
  print('');

  // Extension methods
  print('Extension Methods:');
  print('hello capitalized: ${'hello'.capitalize()}');
  print('hello reversed: ${'hello'.reverse()}');
  print('');

  // String interpolation
  print('String Interpolation:');
  final name = 'World';
  print('Hello, $name!');
  print('1 + 1 = ${1 + 1}');
  print('');

  // Null safety
  print('Null Safety:');
  String? nullableString;
  print('Nullable string: $nullableString');
  nullableString ??= 'default value';
  print('After null-coalescing: $nullableString');
  print('');

  // Collections
  print('Collections:');
  final list = [1, 2, 3, 4, 5];
  print('List: $list');
  print('First: ${list.first}');
  print('Last: ${list.last}');
  print('Length: ${list.length}');
  print('Reversed: ${list.reversed.toList()}');

  final set = {1, 2, 3, 4, 5};
  print('Set: $set');

  final map = {'one': 1, 'two': 2, 'three': 3};
  print('Map: $map');
  print('Value for key "two": ${map['two']}');
  print('');

  // Cascade notation
  print('Cascade Notation:');
  final builder = StringBuilder()
    ..write('Hello')
    ..write(' ')
    ..write('World');
  print(builder.toString());
  print('');

  // Enum
  print('Enum:');
  for (var status in Status.values) {
    print('Status: $status (${status.index})');
  }
  print('');

  // Mixin
  print('Mixin:');
  final userService = UserService();
  userService.createUser('Alice');
  print('');

  // Async/await example
  print('Async/Await:');
  fetchData().then((data) {
    print(data);
  });
  print('');

  // Stream example
  print('Stream:');
  countStream(3).listen((value) {
    print('Stream value: $value');
  });
}

// StringBuilder class
class StringBuilder {
  final _buffer = StringBuffer();

  void write(String text) {
    _buffer.write(text);
  }

  String toString() => _buffer.toString();
}
