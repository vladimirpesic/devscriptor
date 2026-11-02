// Comprehensive F# test file

open System

// Record type for User
type User = {
    Id: int
    Username: string
    Email: string
    Age: int
    IsActive: bool
}

// Discriminated union for Status
type Status =
    | Pending
    | Processing
    | Completed
    | Cancelled

// Create user function
let createUser id username email age isActive =
    { Id = id; Username = username; Email = email; Age = age; IsActive = isActive }

let defaultUser id username email age =
    createUser id username email age true

// Utility functions
let isAdult age = age >= 18

let findUser users id =
    users |> List.tryFind (fun u -> u.Id = id)

let filterActiveUsers users =
    users |> List.filter (fun u -> u.IsActive)

let mapUserNames users =
    users |> List.map (fun u -> u.Username)

let sumAges users =
    users |> List.sumBy (fun u -> u.Age)

let averageAge users =
    match users with
    | [] -> 0.0
    | _ -> (sumAges users |> float) / (List.length users |> float)

// Shape types
type Shape =
    | Circle of float
    | Rectangle of float * float
    | Triangle of float * float * float

let area shape =
    match shape with
    | Circle radius -> Math.PI * radius * radius
    | Rectangle (width, height) -> width * height
    | Triangle (baseHeight, height, _) -> 0.5 * baseHeight * height

let describe shape =
    match shape with
    | Circle radius -> sprintf "Circle with radius %.2f" radius
    | Rectangle (width, height) -> sprintf "Rectangle %.2fx%.2f" width height
    | Triangle (baseHeight, height, side3) -> sprintf "Triangle %.2f-%.2f-%.2f" baseHeight height side3

// Recursion
let rec factorial n =
    if n <= 1 then 1
    else n * factorial (n - 1)

let rec fibonacci n =
    if n <= 1 then n
    else fibonacci (n - 1) + fibonacci (n - 2)

let rec sumList list =
    match list with
    | [] -> 0
    | head :: tail -> head + sumList tail

// Higher-order functions
let mapList list fn =
    list |> List.map fn

let filterList list predicate =
    list |> List.filter predicate

let reduceList list initial fn =
    list |> List.fold fn initial

// Error handling with Result type
type Result<'T, 'TError> =
    | Success of 'T
    | Error of 'TError

let safeDivide numerator denominator =
    if denominator = 0.0 then
        Error "Division by zero"
    else
        Success (numerator / denominator)

// Active patterns
let (|Even|Odd|) n =
    if n % 2 = 0 then Even else Odd

// String operations
let greet name = sprintf "Hello, %s!" name

let formatUserInfo user =
    sprintf
        """User Information:
        ID: %d
        Name: %s
        Email: %s
        Age: %d
        Active: %b"""
        user.Id user.Username user.Email user.Age user.IsActive

// Computation expression builder
type MaybeBuilder() =
    member _.Bind(opt, f) =
        match opt with
        | Some x -> f x
        | None -> None
    member _.Return(x) = Some x
    member _.ReturnFrom(x) = x

let maybe = MaybeBuilder()

// Option operations
let divideSafe numerator denominator =
    if denominator = 0 then None
    else Some (numerator / denominator)

// Main function
[<EntryPoint>]
let main argv =
    printfn "=== F# Test Program ===\n"

    // Create sample users
    let users =
        [ defaultUser 1 "Alice" "alice@example.com" 25
          defaultUser 2 "Bob" "bob@example.com" 30
          createUser 3 "Charlie" "charlie@example.com" 17 false ]

    // Print user information
    printfn "Users:"
    users
    |> List.iter (fun user ->
        printfn "%s\n" (formatUserInfo user))

    // Filter active users
    printfn "\nActive Users:"
    users
    |> filterActiveUsers
    |> List.iter (fun user -> printfn "%s" user.Username)

    // Adult users
    printfn "\nAdult Users:"
    users
    |> List.filter (fun u -> isAdult u.Age)
    |> List.iter (fun user -> printfn "%s" user.Username)

    // Find specific user
    printfn "\nFinding user with ID 2:"
    match findUser users 2 with
    | Some user -> printfn "Found: %s" user.Username
    | None -> printfn "User not found"

    // Calculations
    printfn "\nCalculations:"
    printfn "Total ages: %d" (sumAges users)
    printfn "Average age: %.2f" (averageAge users)

    // Factorial and Fibonacci
    printfn "\nFactorial 5: %d" (factorial 5)
    printfn "Fibonacci 10: %d" (fibonacci 10)

    // Shapes
    printfn "\nShapes:"
    let circle = Circle 5.0
    let rectangle = Rectangle (3.0, 4.0)
    let triangle = Triangle (3.0, 4.0, 5.0)

    printfn "%s with area %.2f" (describe circle) (area circle)
    printfn "%s with area %.2f" (describe rectangle) (area rectangle)
    printfn "%s with area %.2f" (describe triangle) (area triangle)

    // Division
    printfn "\nDivision:"
    match safeDivide 10.0 2.0 with
    | Success result -> printfn "10 / 2 = %.2f" result
    | Error err -> printfn "Error: %s" err

    match safeDivide 10.0 0.0 with
    | Success result -> printfn "10 / 0 = %.2f" result
    | Error err -> printfn "Error: %s" err

    // Higher-order functions
    printfn "\nHigher-order Functions:"
    let numbers = [1; 2; 3; 4; 5]
    let doubled = numbers |> List.map (fun x -> x * 2)
    printfn "Doubled: %A" doubled

    let evens = numbers |> List.filter (fun x -> x % 2 = 0)
    printfn "Even numbers: %A" evens

    let sum = numbers |> List.fold (+) 0
    printfn "Sum: %d" sum

    // Active pattern
    printfn "\nActive Pattern:"
    let testNumbers = [1; 2; 3; 4; 5]
    testNumbers |> List.iter (fun n ->
        match n with
        | Even -> printfn "%d is Even" n
        | Odd -> printfn "%d is Odd" n)

    // Option handling
    printfn "\nOption Handling:"
    let result1 = divideSafe 10.0 2.0
    match result1 with
    | Some value -> printfn "10 / 2 = %.2f" value
    | None -> printfn "Division failed"

    let result2 = divideSafe 10.0 0.0
    match result2 with
    | Some value -> printfn "10 / 0 = %.2f" value
    | None -> printfn "Division failed"

    // Maybe computation expression
    printfn "\nMaybe Computation Expression:"
    let maybeResult =
        maybe {
            let! x = divideSafe 10.0 2.0
            let! y = divideSafe x 2.0
            return y
        }
    match maybeResult with
    | Some value -> printfn "Result: %.2f" value
    | None -> printfn "Computation failed"

    // List operations
    printfn "\nList Operations:"
    printfn "Numbers: %A" numbers
    printfn "Head: %d" numbers.Head
    printfn "Tail: %A" numbers.Tail
    printfn "Length: %d" numbers.Length
    printfn "Reversed: %A" (List.rev numbers)
    printfn "Sorted: %A" (List.sort numbers)

    // Array operations
    printfn "\nArray Operations:"
    let arr = [| 1; 2; 3; 4; 5 |]
    printfn "Array: %A" arr
    printfn "First element: %d" arr.[0]
    printfn "Last element: %d" arr.[arr.Length - 1]
    printfn "Slice [1..3]: %A" arr.[1..3]

    // Sequences
    printfn "\nSequences:"
    let seq1 = seq { 1..10 }
    printfn "Sequence 1..10: %A" (Seq.toList seq1)

    let squares = seq { for x in 1..5 -> x * x }
    printfn "Squares: %A" (Seq.toList squares)

    // Map operations
    printfn "\nMap Operations:"
    let myMap =
        [ ("one", 1)
          ("two", 2)
          ("three", 3) ]
        |> Map.ofList

    printfn "Map: %A" myMap
    printfn "Value for 'two': %d" myMap.["two"]
    printfn "Contains 'four': %b" (myMap.ContainsKey "four")

    let updatedMap = myMap.Add("four", 4)
    printfn "Updated map: %A" updatedMap

    // String operations
    printfn "\nString Operations:"
    let text = "Hello, World!"
    printfn "Uppercase: %s" text.ToUpper()
    printfn "Lowercase: %s" text.ToLower()
    printfn "Length: %d" text.Length
    printfn "Substring [0..5]: %s" text.[0..5]
    printfn "Split: %A" (text.Split([| ',' |]))

    // Pattern matching on records
    printfn "\nPattern Matching on Records:"
    let classifyUser user =
        match user with
        | { IsActive = true; Age = age } when age >= 18 -> "Active adult"
        | { IsActive = true } -> "Active minor"
        | { IsActive = false } -> "Inactive"

    users |> List.iter (fun user ->
        printfn "%s: %s" user.Username (classifyUser user))

    // Discriminated union pattern matching
    printfn "\nStatus Pattern Matching:"
    let matchStatus status =
        match status with
        | Pending -> "Order is pending"
        | Processing -> "Order is being processed"
        | Completed -> "Order completed successfully"
        | Cancelled -> "Order was cancelled"

    let statuses = [Pending; Processing; Completed; Cancelled]
    statuses |> List.iter (fun status ->
        printfn "%A: %s" status (matchStatus status))

    // Function composition
    printfn "\nFunction Composition:"
    let addOne x = x + 1
    let multiplyByTwo x = x * 2
    let composed = addOne >> multiplyByTwo
    printfn "(3 + 1) * 2 = %d" (composed 3)

    // Pipeline operator
    printfn "\nPipeline Operator:"
    let result =
        [1; 2; 3; 4; 5]
        |> List.filter (fun x -> x % 2 = 0)
        |> List.map (fun x -> x * x)
        |> List.sum
    printfn "Sum of squares of even numbers: %d" result

    // Async example
    async {
        printfn "\nAsync Operation:"
        do! Async.Sleep 100
        printfn "Async operation completed"
    } |> Async.RunSynchronously

    0 // Return an integer exit code
