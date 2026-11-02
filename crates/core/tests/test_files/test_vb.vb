' Comprehensive VB.NET test file containing all entities and relationships.

Imports System
Imports System.Collections.Generic
Imports System.Linq
Imports System.Threading.Tasks
Imports System.IO
Imports System.Text

' ============================================================================
' Module-level constants and enums
' ============================================================================

Module GlobalConstants
    Public Const PI As Double = 3.14159265359
    Public Const MAX_SIZE As Integer = 1000
    Public Const DEFAULT_TIMEOUT As Integer = 30000

    ' Enum definition
    Public Enum Color
        Red
        Green
        Blue
        Yellow
        Purple
    End Enum

    ' Enum with explicit values
    Public Enum Priority As Integer
        Low = 1
        Normal = 5
        High = 10
        Critical = 100
    End Enum

    ' Enum with Flags attribute
    <Flags()>
    Public Enum Permissions
        None = 0
        Read = 1
        Write = 2
        Execute = 4
        Delete = 8
        All = Read Or Write Or Execute Or Delete
    End Enum
End Module

' ============================================================================
' Structures (Value Types)
' ============================================================================

Public Structure Point
    Public X As Double
    Public Y As Double

    ' Constructor
    Public Sub New(x As Double, y As Double)
        Me.X = x
        Me.Y = y
    End Sub

    ' Method
    Public Function DistanceTo(other As Point) As Double
        Return Math.Sqrt((X - other.X) ^ 2 + (Y - other.Y) ^ 2)
    End Function

    ' Property
    Public ReadOnly Property Magnitude As Double
        Get
            Return Math.Sqrt(X ^ 2 + Y ^ 2)
        End Get
    End Property

    ' Operator overloading
    Public Shared Operator +(p1 As Point, p2 As Point) As Point
        Return New Point(p1.X + p2.X, p1.Y + p2.Y)
    End Operator

    Public Shared Operator -(p1 As Point, p2 As Point) As Point
        Return New Point(p1.X - p2.X, p1.Y - p2.Y)
    End Operator

    ' Override ToString
    Public Overrides Function ToString() As String
        Return $"Point({X}, {Y})"
    End Function
End Structure

Public Structure Rectangle
    Public TopLeft As Point
    Public BottomRight As Point

    Public Sub New(topLeft As Point, bottomRight As Point)
        Me.TopLeft = topLeft
        Me.BottomRight = bottomRight
    End Sub

    Public ReadOnly Property Width As Double
        Get
            Return Math.Abs(BottomRight.X - TopLeft.X)
        End Get
    End Property

    Public ReadOnly Property Height As Double
        Get
            Return Math.Abs(BottomRight.Y - TopLeft.Y)
        End Get
    End Property

    Public ReadOnly Property Area As Double
        Get
            Return Width * Height
        End Get
    End Property

    Public Function Contains(p As Point) As Boolean
        Return p.X >= TopLeft.X AndAlso p.X <= BottomRight.X AndAlso
               p.Y >= TopLeft.Y AndAlso p.Y <= BottomRight.Y
    End Function
End Structure

' ============================================================================
' Interfaces
' ============================================================================

Public Interface IShape
    ReadOnly Property Name As String
    Function CalculateArea() As Double
    Function CalculatePerimeter() As Double
    Sub Draw()
End Interface

Public Interface IDrawable
    Sub Render()
    Property Color As Color
End Interface

Public Interface ISerializable
    Function Serialize() As String
    Sub Deserialize(data As String)
End Interface

Public Interface IGeneric(Of T)
    Property Value As T
    Function Process(data As T) As T
End Interface

' ============================================================================
' Abstract Base Class
' ============================================================================

Public MustInherit Class ShapeBase
    Implements IShape

    Protected _name As String
    Protected _color As Color

    Public Sub New(name As String)
        _name = name
        _color = Color.Blue
    End Sub

    Public ReadOnly Property Name As String Implements IShape.Name
        Get
            Return _name
        End Get
    End Property

    Public Property ShapeColor As Color
        Get
            Return _color
        End Get
        Set(value As Color)
            _color = value
        End Set
    End Property

    ' Abstract methods
    Public MustOverride Function CalculateArea() As Double Implements IShape.CalculateArea
    Public MustOverride Function CalculatePerimeter() As Double Implements IShape.CalculatePerimeter

    ' Virtual method
    Public Overridable Sub Draw() Implements IShape.Draw
        Console.WriteLine($"Drawing {_name}")
    End Sub

    ' Concrete method
    Public Sub PrintInfo()
        Console.WriteLine($"Shape: {_name}")
        Console.WriteLine($"Color: {_color}")
        Console.WriteLine($"Area: {CalculateArea()}")
    End Sub
End Class

' ============================================================================
' Concrete Classes
' ============================================================================

Public Class Circle
    Inherits ShapeBase
    Implements IDrawable, ISerializable

    Private _radius As Double

    Public Sub New(radius As Double)
        MyBase.New("Circle")
        _radius = radius
    End Sub

    Public Property Radius As Double
        Get
            Return _radius
        End Get
        Set(value As Double)
            If value > 0 Then
                _radius = value
            End If
        End Set
    End Property

    Public Overrides Function CalculateArea() As Double
        Return PI * _radius * _radius
    End Function

    Public Overrides Function CalculatePerimeter() As Double
        Return 2 * PI * _radius
    End Function

    Public Overrides Sub Draw()
        Console.WriteLine($"Drawing a circle with radius {_radius}")
    End Sub

    Public Sub Render() Implements IDrawable.Render
        Draw()
    End Sub

    Public Property RenderColor As Color Implements IDrawable.Color
        Get
            Return ShapeColor
        End Get
        Set(value As Color)
            ShapeColor = value
        End Set
    End Property

    Public Function Serialize() As String Implements ISerializable.Serialize
        Return $"Circle:{{radius:{_radius},color:{ShapeColor}}}"
    End Function

    Public Sub Deserialize(data As String) Implements ISerializable.Deserialize
        ' Parse and deserialize data
    End Sub
End Class

Public Class RectangleShape
    Inherits ShapeBase
    Implements IDrawable

    Private _width As Double
    Private _height As Double

    Public Sub New(width As Double, height As Double)
        MyBase.New("Rectangle")
        _width = width
        _height = height
    End Sub

    Public Property Width As Double
        Get
            Return _width
        End Get
        Set(value As Double)
            _width = value
        End Set
    End Property

    Public Property Height As Double
        Get
            Return _height
        End Get
        Set(value As Double)
            _height = value
        End Set
    End Property

    Public Overrides Function CalculateArea() As Double
        Return _width * _height
    End Function

    Public Overrides Function CalculatePerimeter() As Double
        Return 2 * (_width + _height)
    End Function

    Public Sub Render() Implements IDrawable.Render
        Console.WriteLine($"Rendering rectangle {_width}x{_height}")
    End Sub

    Public Property RenderColor As Color Implements IDrawable.Color
        Get
            Return ShapeColor
        End Get
        Set(value As Color)
            ShapeColor = value
        End Set
    End Property
End Class

' Generic class
Public Class Container(Of T)
    Private _items As New List(Of T)

    Public Sub Add(item As T)
        _items.Add(item)
    End Sub

    Public Function GetItem(index As Integer) As T
        Return _items(index)
    End Function

    Public ReadOnly Property Count As Integer
        Get
            Return _items.Count
        End Get
    End Property

    Public Function GetAll() As List(Of T)
        Return New List(Of T)(_items)
    End Function

    Default Public ReadOnly Property Item(index As Integer) As T
        Get
            Return _items(index)
        End Get
    End Property
End Class

' ============================================================================
' Events and Delegates
' ============================================================================

Public Class EventPublisher
    ' Event declaration
    Public Event DataChanged(sender As Object, e As EventArgs)
    Public Event ValueUpdated(value As Integer)

    Private _value As Integer

    Public Property Value As Integer
        Get
            Return _value
        End Get
        Set(value As Integer)
            _value = value
            RaiseEvent DataChanged(Me, EventArgs.Empty)
            RaiseEvent ValueUpdated(_value)
        End Set
    End Property

    Public Sub TriggerEvent()
        RaiseEvent DataChanged(Me, EventArgs.Empty)
    End Sub
End Class

' Custom EventArgs
Public Class CustomEventArgs
    Inherits EventArgs

    Public Property Message As String
    Public Property Timestamp As DateTime

    Public Sub New(message As String)
        Me.Message = message
        Me.Timestamp = DateTime.Now
    End Sub
End Class

' ============================================================================
' Extension Methods
' ============================================================================

Public Module StringExtensions
    <Runtime.CompilerServices.Extension()>
    Public Function ReverseString(str As String) As String
        Dim charArray = str.ToCharArray()
        Array.Reverse(charArray)
        Return New String(charArray)
    End Function

    <Runtime.CompilerServices.Extension()>
    Public Function WordCount(str As String) As Integer
        Return str.Split(New Char() {" "c, "."c, ","c}, StringSplitOptions.RemoveEmptyEntries).Length
    End Function
End Module

' ============================================================================
' Module Functions and Subroutines
' ============================================================================

Module TestModule
    ' Simple function
    Public Function Add(a As Integer, b As Integer) As Integer
        Return a + b
    End Function

    ' Function with optional parameters
    Public Function Greet(name As String, Optional greeting As String = "Hello") As String
        Return $"{greeting}, {name}!"
    End Function

    ' Function with ByRef parameter
    Public Sub Swap(Of T)(ByRef a As T, ByRef b As T)
        Dim temp = a
        a = b
        b = temp
    End Sub

    ' Recursive function
    Public Function Factorial(n As Integer) As Integer
        If n <= 1 Then
            Return 1
        Else
            Return n * Factorial(n - 1)
        End If
    End Function

    ' Async function
    Public Async Function FetchDataAsync(url As String) As Task(Of String)
        Await Task.Delay(100) ' Simulate network delay
        Return $"Data from {url}"
    End Function

    ' Function returning tuple
    Public Function DivideWithRemainder(dividend As Integer, divisor As Integer) As (quotient As Integer, remainder As Integer)
        Dim q = dividend \ divisor
        Dim r = dividend Mod divisor
        Return (q, r)
    End Function

    ' LINQ example
    Public Function GetEvenNumbers(numbers As List(Of Integer)) As List(Of Integer)
        Return numbers.Where(Function(n) n Mod 2 = 0).ToList()
    End Function

    ' Generic function
    Public Function FindMax(Of T As IComparable)(items As List(Of T)) As T
        Dim max = items(0)
        For Each item In items.Skip(1)
            If item.CompareTo(max) > 0 Then
                max = item
            End If
        Next
        Return max
    End Function

    ' Using statement example
    Public Sub WriteToFile(filename As String, content As String)
        Using writer As New StreamWriter(filename)
            writer.WriteLine(content)
        End Using
    End Sub

    ' Try-catch-finally
    Public Function SafeDivide(a As Double, b As Double) As Double
        Try
            Return a / b
        Catch ex As DivideByZeroException
            Console.WriteLine("Cannot divide by zero")
            Return Double.NaN
        Catch ex As Exception
            Console.WriteLine($"Error: {ex.Message}")
            Throw
        Finally
            Console.WriteLine("Division attempted")
        End Try
    End Function

    ' Lambda expression
    Public Function CreateMultiplier(factor As Integer) As Func(Of Integer, Integer)
        Return Function(x) x * factor
    End Function

    ' Main entry point
    Sub Main()
        Console.WriteLine("VB.NET Comprehensive Test")
        Console.WriteLine("=========================")

        ' Test constants and enums
        Console.WriteLine($"PI = {PI}")
        Console.WriteLine($"Priority.Normal = {Priority.Normal}")

        ' Test structures
        Dim p1 As New Point(3, 4)
        Dim p2 As New Point(0, 0)
        Console.WriteLine($"Point p1: {p1}")
        Console.WriteLine($"Magnitude: {p1.Magnitude}")
        Console.WriteLine($"Distance: {p1.DistanceTo(p2)}")

        Dim p3 = p1 + New Point(1, 1)
        Console.WriteLine($"p1 + (1,1) = {p3}")

        ' Test classes
        Dim circle As New Circle(5.0)
        Console.WriteLine($"Circle area: {circle.CalculateArea()}")
        Console.WriteLine($"Circle perimeter: {circle.CalculatePerimeter()}")
        circle.Draw()

        Dim rect As New RectangleShape(4, 6)
        Console.WriteLine($"Rectangle area: {rect.CalculateArea()}")

        ' Test polymorphism
        Dim shapes As New List(Of ShapeBase) From {circle, rect}
        For Each shape In shapes
            shape.PrintInfo()
        Next

        ' Test interfaces
        Dim drawable As IDrawable = circle
        drawable.Render()

        ' Test generic class
        Dim container As New Container(Of String)
        container.Add("First")
        container.Add("Second")
        Console.WriteLine($"Container count: {container.Count}")

        ' Test functions
        Console.WriteLine($"Add(5, 3) = {Add(5, 3)}")
        Console.WriteLine(Greet("World"))
        Console.WriteLine(Greet("VB.NET", "Welcome to"))
        Console.WriteLine($"Factorial(5) = {Factorial(5)}")

        ' Test swap
        Dim x = 10, y = 20
        Swap(x, y)
        Console.WriteLine($"After swap: x={x}, y={y}")

        ' Test tuple return
        Dim result = DivideWithRemainder(17, 5)
        Console.WriteLine($"17 / 5 = {result.quotient} remainder {result.remainder}")

        ' Test extension methods
        Dim testString = "Hello World"
        Console.WriteLine($"Reversed: {testString.ReverseString()}")
        Console.WriteLine($"Word count: {testString.WordCount()}")

        ' Test events
        Dim publisher As New EventPublisher()
        AddHandler publisher.DataChanged, Sub(s, e) Console.WriteLine("Data changed!")
        publisher.Value = 42

        Console.WriteLine()
        Console.WriteLine("All tests completed successfully!")
    End Sub
End Module
