"""
Comprehensive Python test file containing all entities and relationships.
"""

# Imports
import os
import sys
from typing import List, Dict, Optional, Union, TypeVar, Generic, Protocol, runtime_checkable
from enum import Enum, IntEnum, Flag, IntFlag
from dataclasses import dataclass, field
from abc import ABC, abstractmethod, ABCMeta

# Type variables for generics
T = TypeVar("T")
U = TypeVar("U", bound=int)


# Decorator (annotation)
def my_decorator(func):
    """Decorator function."""

    def wrapper(*args, **kwargs):
        print(f"Calling {func.__name__}")
        return func(*args, **kwargs)

    return wrapper


# Class decorator
def class_decorator(cls):
    """Class decorator."""
    original_new = cls.__new__

    @classmethod
    def custom_new(cls, *args, **kwargs):
        print(f"Creating instance of {cls.__name__}")
        return original_new(cls)

    cls.__new__ = custom_new
    return cls


# Enum with all enum features
class Color(Enum):
    """Color enum."""

    RED = 1
    GREEN = 2
    BLUE = 3

    def description(self) -> str:
        """Get description."""
        return f"This is {self.name}"


class Priority(IntEnum):
    """Priority enum with integer values."""

    LOW = 1
    MEDIUM = 2
    HIGH = 3


# Flag enum
class Permission(Flag):
    """Permission flag."""

    READ = 1
    WRITE = 2
    EXECUTE = 4


# IntFlag enum
class FileMode(IntFlag):
    """File mode flag."""

    READ = 1
    WRITE = 2
    EXECUTE = 4


# Protocol (interface-like)
@runtime_checkable
class Drawable(Protocol):
    """Drawable protocol."""

    def draw(self) -> None:
        """Draw the object."""
        ...


# Abstract base class with inheritance
class Shape(ABC):
    """Abstract shape class."""

    def __init__(self, name: str):
        self.name = name

    @abstractmethod
    def area(self) -> float:
        """Calculate area."""
        pass

    @abstractmethod
    def perimeter(self) -> float:
        """Calculate perimeter."""
        pass

    def describe(self) -> str:
        """Describe the shape."""
        return f"Shape: {self.name}"


# Class inheriting from ABC
class Circle(Shape):
    """Circle shape."""

    def __init__(self, name: str, radius: float):
        super().__init__(name)
        self.radius = radius

    def area(self) -> float:
        """Calculate circle area."""
        return 3.14159 * self.radius**2

    def perimeter(self) -> float:
        """Calculate circle perimeter."""
        return 2 * 3.14159 * self.radius


# Multiple inheritance
class ColoredCircle(Circle):
    """Colored circle with multiple inheritance."""

    def __init__(self, name: str, radius: float, color: Color):
        super().__init__(name, radius)
        self.color = color

    def area(self) -> float:
        """Calculate area with color modifier."""
        return super().area()


# Dataclass
@dataclass
class Person:
    """Person dataclass."""

    name: str
    age: int
    email: str = "default@example.com"
    addresses: List[str] = field(default_factory=list)


# Generic dataclass
@dataclass
class Container(Generic[T]):
    """Generic container."""

    items: List[T]
    size: int = 0

    def add(self, item: T) -> None:
        """Add item."""
        self.items.append(item)
        self.size += 1


# Regular function with type hints
def greet(name: str) -> str:
    """Greet someone."""
    return f"Hello, {name}!"


# Function with default arguments
def calculate(a: int, b: int, operation: str = "add") -> float:
    """Calculate based on operation."""
    if operation == "add":
        return float(a + b)
    elif operation == "subtract":
        return float(a - b)
    elif operation == "multiply":
        return float(a * b)
    elif operation == "divide":
        return float(a / b) if b != 0 else 0.0
    return 0.0


# Function with *args and **kwargs
def flexible_function(*args, **kwargs) -> Dict[str, any]:
    """Flexible function."""
    result = {"args": args, "kwargs": kwargs}
    return result


# Async function
import asyncio


async def async_fetch(url: str) -> str:
    """Async fetch data."""
    await asyncio.sleep(0.1)
    return f"Data from {url}"


# Async function with multiple awaits
async def async_process_multiple(urls: List[str]) -> List[str]:
    """Process multiple URLs asynchronously."""
    tasks = [async_fetch(url) for url in urls]
    results = await asyncio.gather(*tasks)
    return results


# Generator function
def number_generator(n: int):
    """Generate numbers."""
    for i in range(n):
        yield i


# Lambda function (anonymous)
square = lambda x: x**2


# Nested function
def outer_function(x: int) -> int:
    """Outer function with nested function."""

    def inner_function(y: int) -> int:
        """Inner function."""
        return y * 2

    return inner_function(x)


# Function with pattern matching (Python 3.10+)
def match_pattern(value) -> str:
    """Match pattern example."""
    match value:
        case int():
            return f"Integer: {value}"
        case str():
            return f"String: {value}"
        case list():
            return f"List with {len(value)} items"
        case _:
            return "Unknown type"


# Method in class
class Calculator:
    """Calculator class."""

    def __init__(self):
        self.history: List[str] = []

    def add(self, a: float, b: float) -> float:
        """Add two numbers."""
        result = a + b
        self.history.append(f"{a} + {b} = {result}")
        return result

    def subtract(self, a: float, b: float) -> float:
        """Subtract two numbers."""
        result = a - b
        self.history.append(f"{a} - {b} = {result}")
        return result

    def get_history(self) -> List[str]:
        """Get calculation history."""
        return self.history.copy()


# Class method and static method
class Utility:
    """Utility class."""

    count = 0

    def __init__(self):
        Utility.count += 1

    @classmethod
    def get_count(cls) -> int:
        """Get instance count."""
        return cls.count

    @staticmethod
    def is_even(n: int) -> bool:
        """Check if number is even."""
        return n % 2 == 0


# Property
class Temperature:
    """Temperature class with property."""

    def __init__(self, celsius: float = 0.0):
        self._celsius = celsius

    @property
    def celsius(self) -> float:
        """Get celsius."""
        return self._celsius

    @celsius.setter
    def celsius(self, value: float):
        """Set celsius."""
        self._celsius = value

    @property
    def fahrenheit(self) -> float:
        """Get fahrenheit."""
        return self._celsius * 9 / 5 + 32

    @fahrenheit.setter
    def fahrenheit(self, value: float):
        """Set fahrenheit."""
        self._celsius = (value - 32) * 5 / 9


# Context manager
class FileManager:
    """File manager context manager."""

    def __init__(self, filename: str, mode: str = "r"):
        self.filename = filename
        self.mode = mode
        self.file = None

    def __enter__(self):
        """Enter context."""
        self.file = open(self.filename, self.mode)
        return self.file

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Exit context."""
        if self.file:
            self.file.close()
        return False


# Union type
def process_value(value: Union[int, str, float]) -> str:
    """Process union type value."""
    return str(value)


# Optional type
def find_user(user_id: int) -> Optional[Person]:
    """Find user by ID."""
    if user_id == 1:
        return Person("John", 30)
    return None


# Function calls and relationships
def main():
    """Main function demonstrating all entities."""

    # Create instances
    circle = Circle("MyCircle", 5.0)
    colored_circle = ColoredCircle("RedCircle", 3.0, Color.RED)
    person = Person("Alice", 25, "alice@example.com")
    calculator = Calculator()

    # Function calls
    print(greet("World"))
    print(calculate(10, 5))
    print(calculate(10, 5, "multiply"))

    # Method calls
    print(circle.area())
    print(circle.perimeter())
    print(circle.describe())

    print(colored_circle.area())

    print(calculator.add(10, 5))
    print(calculator.subtract(10, 3))

    # Class method and static method calls
    print(Utility.get_count())
    print(Utility.is_even(4))

    # Property access
    temp = Temperature(25.0)
    print(temp.celsius)
    print(temp.fahrenheit)
    temp.fahrenheit = 86.0
    print(temp.celsius)

    # Enum usage
    print(Color.RED.description())
    print(Priority.HIGH)

    # Generic usage
    container = Container[int]([])
    container.add(1)
    container.add(2)
    print(container.size)

    # Generator usage
    gen = number_generator(5)
    print(list(gen))

    # Lambda usage
    print(square(5))

    # Pattern matching
    print(match_pattern(42))
    print(match_pattern("hello"))
    print(match_pattern([1, 2, 3]))

    # Optional handling
    user = find_user(1)
    if user:
        print(f"Found: {user.name}")

    # Context manager usage
    with FileManager("test.txt", "w") as f:
        f.write("Hello, World!")


if __name__ == "__main__":
    main()
