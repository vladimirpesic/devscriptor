# frozen_string_literal: true

# Comprehensive Ruby test file containing all entities and relationships.

# Module (namespace)
module Geometry
  class Point
    attr_accessor :x, :y
    
    def initialize(x, y)
      @x = x
      @y = y
    end
  end
end

# Module with mixin
module Loggable
  def log(message)
    puts "Log: #{message}"
  end
end

# Class with inheritance and mixins
class Shape
  include Loggable
  
  attr_reader :name
  
  def initialize(name)
    @name = name
  end
  
  def describe
    puts "Shape: #{@name}"
  end
end

# Class inheriting from parent
class Circle < Shape
  attr_reader :radius
  
  def initialize(name, radius)
    super(name)
    @radius = radius
  end
  
  def area
    Math::PI * @radius ** 2
  end
  
  def perimeter
    2 * Math::PI * @radius
  end
  
  def draw
    puts "Drawing circle: #{@name}"
    log("Circle drawn")
  end
end

# Singleton class
class Singleton
  include Singleton
  
  def initialize
    @counter = 0
  end
  
  def increment
    @counter += 1
  end
  
  def count
    @counter
  end
end

# Class with accessors, class methods, and instance methods
class Person
  attr_accessor :name, :age, :email
  attr_reader :id
  
  @@count = 0
  
  def initialize(name, age, email)
    @id = @@count + 1
    @name = name
    @age = age
    @email = email
    @@count += 1
  end
  
  def greet
    puts "Hello, I'm #{@name}"
  end
  
  def self.create_default
    new('Unknown', 0, 'unknown@example.com')
  end
  
  def self.count
    @@count
  end
  
  def to_s
    "Person: #{@name}, age #{@age}"
  end
  
  def inspect
    "Person(#{@name}, #{@age}, #{@email})"
  end
end

# Module with constants
module Constants
  MAX_SIZE = 100
  MIN_SIZE = 1
  VERSION = '1.0.0'
end

# Class with class variables
class Counter
  @@total = 0
  
  def self.total
    @@total
  end
  
  def initialize
    @@total += 1
  end
end

# Method with default arguments
def greet(name, greeting = 'Hello')
  puts "#{greeting}, #{name}!"
end

# Method with variable arguments (*args)
def sum(*numbers)
  numbers.reduce(0, :+)
end

# Method with keyword arguments (**kwargs)
def create_object(**attributes)
  attributes
end

# Method with splat and keyword arguments
def flexible(*args, **kwargs)
  puts "Args: #{args.inspect}"
  puts "Kwargs: #{kwargs.inspect}"
end

# Block usage
def with_block
  yield if block_given?
end

# Lambda
square = ->(x) { x ** 2 }
add = lambda { |a, b| a + b }

# Proc
multiply = proc { |a, b| a * b }

# Method returning a proc
def create_multiplier(factor)
  proc { |x| x * factor }
end

# Class with initialize and custom constructors
class Rectangle
  attr_reader :width, :height
  
  def initialize(width, height)
    @width = width
    @height = height
  end
  
  def self.square(side)
    new(side, side)
  end
  
  def area
    @width * @height
  end
end

# Exception handling
class CustomError < StandardError
  def initialize(message = 'Something went wrong')
    super(message)
  end
end

def risky_operation
  raise CustomError, 'Operation failed'
rescue CustomError => e
  puts "Rescued: #{e.message}"
  raise if @retry_count
ensure
  puts 'Cleanup code'
end

# Case/when statement (pattern matching)
def match_pattern(value)
  case value
  when Integer
    "Integer: #{value}"
  when String
    "String: #{value}"
  when Array
    "Array with #{value.size} items"
  when true, false
    "Boolean: #{value}"
  when nil
    'Nil'
  else
    'Unknown'
  end
end

# Class with method missing
class DynamicObject
  def method_missing(method_name, *args, &block)
    puts "Method #{method_name} called with #{args.inspect}"
  end
  
  def respond_to_missing?(method_name, include_private = false)
    true
  end
end

# Class with define_method
class MetaProgramming
  define_method(:dynamic_method) do |arg|
    puts "Dynamic method called with: #{arg}"
  end
end

# Struct
Point2D = Struct.new(:x, :y, keyword_init: true) do
  def distance_to(other)
    dx = x - other.x
    dy = y - other.y
    Math.sqrt(dx**2 + dy**2)
  end
end

# Class with equality and hash
class Card
  attr_reader :suit, :rank
  
  def initialize(suit, rank)
    @suit = suit
    @rank = rank
  end
  
  def ==(other)
    suit == other.suit && rank == other.rank
  end
  
  def hash
    [suit, rank].hash
  end
  
  def eql?(other)
    self == other
  end
end

# Class with comparison
class Version
  include Comparable
  
  attr_reader :major, :minor, :patch
  
  def initialize(major, minor = 0, patch = 0)
    @major = major
    @minor = minor
    @patch = patch
  end
  
  def <=>(other)
    [major, minor, patch] <=> [other.major, other.minor, other.patch]
  end
  
  def to_s
    "#{major}.#{minor}.#{patch}"
  end
end

# Class with enumerable
class NumberSequence
  include Enumerable
  
  def initialize(start, stop)
    @start = start
    @stop = stop
  end
  
  def each
    return enum_for(__method__) unless block_given?
    
    (@start..@stop).each do |n|
      yield n
    end
  end
end

# Class with iterators
class Iterator
  def initialize(collection)
    @collection = collection
    @index = 0
  end
  
  def next?
    @index < @collection.size
  end
  
  def next
    value = @collection[@index]
    @index += 1
    value
  end
end

# Class with class instance variables
class Config
  @settings = {}
  
  class << self
    attr_accessor :settings
    
    def set(key, value)
      @settings[key] = value
    end
    
    def get(key)
      @settings[key]
    end
  end
end

# Regular expressions
def parse_email(email)
  pattern = /\A[\w+\-.]+@[a-z\d\-]+(\.[a-z]+)*\.[a-z]+\z/i
  match = pattern.match(email)
  match ? match[0] : nil
end

# Class with forwarding
class Wrapper
  def initialize(target)
    @target = target
  end
  
  def method_missing(method, *args, &block)
    @target.send(method, *args, &block)
  end
end

# Class with freeze and dup
class Immutable
  attr_reader :value
  
  def initialize(value)
    @value = value
    freeze
  end
end

# Thread usage
def concurrent_task
  threads = []
  
  threads << Thread.new do
    sleep 0.1
    puts "Thread 1 completed"
  end
  
  threads << Thread.new do
    sleep 0.2
    puts "Thread 2 completed"
  end
  
  threads.each(&:join)
end

# Fiber (coroutines)
def fiber_example
  fiber = Fiber.new do
    puts "Fiber started"
    Fiber.yield 1
    puts "Fiber resumed"
    Fiber.yield 2
    puts "Fiber finishing"
  end
  
  puts fiber.resume
  puts fiber.resume
  puts fiber.resume
end

# Main execution
if __FILE__ == $PROGRAM_NAME
  # Module usage
  point = Geometry::Point.new(3, 4)
  puts "Point: #{point.x}, #{point.y}"
  
  # Inheritance
  circle = Circle.new('MyCircle', 5.0)
  circle.describe
  circle.draw
  puts "Area: #{circle.area}"
  puts "Perimeter: #{circle.perimeter}"
  
  # Singleton
  singleton = Singleton.instance
  singleton.increment
  puts "Singleton count: #{singleton.count}"
  
  # Person class
  person = Person.new('Alice', 25, 'alice@example.com')
  person.greet
  puts person.to_s
  
  default_person = Person.create_default
  puts "Total persons: #{Person.count}"
  
  # Constants
  puts "Max size: #{Constants::MAX_SIZE}"
  puts "Version: #{Constants::VERSION}"
  
  # Methods
  greet('World')
  puts "Sum: #{sum(1, 2, 3, 4, 5)}"
  puts "Object: #{create_object(name: 'Test', value: 42)}"
  
  # Block
  with_block { puts 'Block executed' }
  
  # Lambda and Proc
  puts "Square: #{square.call(5)}"
  puts "Add: #{add.call(3, 4)}"
  puts "Multiply: #{multiply.call(3, 4)}"
  
  doubler = create_multiplier(2)
  puts "Double 5: #{doubler.call(5)}"
  
  # Rectangle
  square = Rectangle.square(5)
  puts "Square area: #{square.area}"
  
  # Exception
  risky_operation
  
  # Pattern matching
  puts match_pattern(42)
  puts match_pattern('hello')
  puts match_pattern([1, 2, 3])
  
  # Dynamic object
  dynamic = DynamicObject.new
  dynamic.unknown_method(1, 2, 3)
  
  # Struct
  p1 = Point2D.new(x: 0, y: 0)
  p2 = Point2D.new(x: 3, y: 4)
  puts "Distance: #{p1.distance_to(p2)}"
  
  # Card
  card1 = Card.new('hearts', 'ace')
  card2 = Card.new('hearts', 'ace')
  puts "Cards equal: #{card1 == card2}"
  
  # Version
  v1 = Version.new(1, 2, 3)
  v2 = Version.new(1, 2, 4)
  puts "Versions: #{v1} < #{v2} = #{v1 < v2}"
  
  # Enumerable
  sequence = NumberSequence.new(1, 5)
  puts "Sum: #{sequence.sum}"
  puts "Select even: #{sequence.select(&:even)}"
  
  # Iterator
  iterator = Iterator.new([1, 2, 3])
  while iterator.next?
    puts iterator.next
  end
  
  # Config
  Config.set(:timeout, 30)
  puts "Timeout: #{Config.get(:timeout)}"
  
  # Regex
  puts "Valid email: #{parse_email('user@example.com')}"
  
  # Thread
  concurrent_task
  
  # Fiber
  fiber_example
end
