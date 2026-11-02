/**
 * Comprehensive TypeScript test file containing all entities and relationships.
 */

// Imports
import { utilityFunction } from './utils';
import * as MathUtils from './math';

// Type definitions
type Name = string;
type Age = number;
type ID = string | number;

// Interface
interface Person {
    name: string;
    age: number;
    email?: string;
}

// Interface with method signatures
interface Drawable {
    draw(): void;
    getArea(): number;
}

// Interface extending another interface
interface Shape extends Drawable {
    name: string;
    perimeter(): number;
}

// Generic interface
interface Repository<T> {
    findById(id: ID): Promise<T | null>;
    findAll(): Promise<T[]>;
    save(entity: T): Promise<T>;
    delete(id: ID): Promise<boolean>;
}

// Enum
enum Color {
    Red = 0,
    Green = 1,
    Blue = 2
}

enum Priority {
    Low = 'low',
    Medium = 'medium',
    High = 'high'
}

// String enum
enum Direction {
    Up = 'UP',
    Down = 'DOWN',
    Left = 'LEFT',
    Right = 'RIGHT'
}

// Union types
type StringOrNumber = string | number;
type Result<T, E = Error> = 
    | { success: true; data: T }
    | { success: false; error: E };

// Intersection types
type Employee = Person & {
    position: string;
    salary: number;
};

// Type guards
function isString(value: unknown): value is string {
    return typeof value === 'string';
}

// Type assertions
const someValue: unknown = 'hello';
const strLength: number = (someValue as string).length;

// Type variables for generics
type TreeNode<T> = {
    value: T;
    left?: TreeNode<T>;
    right?: TreeNode<T>;
};

// Function with type annotations
function greet(name: string): string {
    return `Hello, ${name}!`;
}

// Function with union return type
function parseInput(input: string): number | string {
    const num = parseFloat(input);
    return isNaN(num) ? input : num;
}

// Function with generic types
function identity<T>(arg: T): T {
    return arg;
}

// Function with multiple generics
function pair<T, U>(first: T, second: U): [T, U] {
    return [first, second];
}

// Function with default parameters
function calculate(
    a: number,
    b: number,
    operation: string = 'add'
): number {
    switch (operation) {
        case 'add': return a + b;
        case 'subtract': return a - b;
        case 'multiply': return a * b;
        case 'divide': return a / b;
        default: return 0;
    }
}

// Function with rest parameters
function sumAll(...numbers: number[]): number {
    return numbers.reduce((acc, num) => acc + num, 0);
}

// Function with destructuring parameters
function createUser({ name, age, email = 'default@example.com' }: Person): Person {
    return { name, age, email };
}

// Async function with Promise return type
async function fetchData(url: string): Promise<any> {
    const response = await fetch(url);
    return await response.json();
}

// Async function with typed return
async function fetchUser(id: ID): Promise<Person | null> {
    const data = await fetchData(`/api/users/${id}`);
    return data || null;
}

// Function overloading
function format(value: string): string;
function format(value: number): string;
function format(value: boolean): string;
function format(value: string | number | boolean): string {
    return String(value);
}

// Arrow function with types
const add = (a: number, b: number): number => a + b;
const multiply = (a: number, b: number): number => a * b;

// Class implementing interface
class Circle implements Shape {
    public name: string;
    private radius: number;
    
    constructor(name: string, radius: number) {
        this.name = name;
        this.radius = radius;
    }
    
    draw(): void {
        console.log(`Drawing circle ${this.name}`);
    }
    
    getArea(): number {
        return Math.PI * this.radius ** 2;
    }
    
    perimeter(): number {
        return 2 * Math.PI * this.radius;
    }
}

// Abstract class
abstract class Animal {
    protected name: string;
    
    constructor(name: string) {
        this.name = name;
    }
    
    abstract makeSound(): void;
    
    move(): void {
        console.log(`${this.name} is moving`);
    }
}

// Class extending abstract class
class Dog extends Animal {
    private breed: string;
    
    constructor(name: string, breed: string) {
        super(name);
        this.breed = breed;
    }
    
    makeSound(): void {
        console.log(`${this.name} barks`);
    }
    
    getBreed(): string {
        return this.breed;
    }
}

// Generic class
class Box<T> {
    private content: T;
    
    constructor(content: T) {
        this.content = content;
    }
    
    getContent(): T {
        return this.content;
    }
    
    setContent(content: T): void {
        this.content = content;
    }
}

// Class with access modifiers
class BankAccount {
    private balance: number;
    protected owner: string;
    public accountNumber: string;
    
    constructor(owner: string, accountNumber: string, initialBalance: number = 0) {
        this.owner = owner;
        this.accountNumber = accountNumber;
        this.balance = initialBalance;
    }
    
    public deposit(amount: number): void {
        if (amount > 0) {
            this.balance += amount;
        }
    }
    
    public withdraw(amount: number): void {
        if (amount > 0 && amount <= this.balance) {
            this.balance -= amount;
        }
    }
    
    public getBalance(): number {
        return this.balance;
    }
    
    protected getOwner(): string {
        return this.owner;
    }
}

// Inheritance
class SavingsAccount extends BankAccount {
    private interestRate: number;
    
    constructor(owner: string, accountNumber: string, interestRate: number) {
        super(owner, accountNumber);
        this.interestRate = interestRate;
    }
    
    public applyInterest(): void {
        const balance = this.getBalance();
        const interest = balance * this.interestRate;
        this.deposit(interest);
    }
    
    public getInterestRate(): number {
        return this.interestRate;
    }
}

// Readonly properties
interface ReadonlyUser {
    readonly id: number;
    readonly name: string;
}

// Readonly class properties
class Config {
    public readonly apiUrl: string;
    private apiKey: string;
    
    constructor(apiUrl: string, apiKey: string) {
        this.apiUrl = apiUrl;
        this.apiKey = apiKey;
    }
    
    public getKey(): string {
        return this.apiKey;
    }
}

// Getters and setters
class Temperature {
    private _celsius: number;
    
    constructor(celsius: number = 0) {
        this._celsius = celsius;
    }
    
    get celsius(): number {
        return this._celsius;
    }
    
    set celsius(value: number) {
        this._celsius = value;
    }
    
    get fahrenheit(): number {
        return this._celsius * 9/5 + 32;
    }
    
    set fahrenheit(value: number) {
        this._celsius = (value - 32) * 5/9;
    }
}

// Static members
class Utility {
    private static instanceCount: number = 0;
    
    constructor() {
        Utility.instanceCount++;
    }
    
    public static getInstanceCount(): number {
        return Utility.instanceCount;
    }
    
    public static isEven(n: number): boolean {
        return n % 2 === 0;
    }
}

// Decorators (if experimentalDecorators enabled)
function sealed(constructor: Function) {
    Object.seal(constructor);
    Object.seal(constructor.prototype);
}

@sealed
class SealedClass {
    constructor() {
        console.log('Sealed class instantiated');
    }
}

// Method decorator
function log(target: any, propertyKey: string, descriptor: PropertyDescriptor) {
    const originalMethod = descriptor.value;
    
    descriptor.value = function(...args: any[]) {
        console.log(`Calling ${propertyKey} with args:`, args);
        const result = originalMethod.apply(this, args);
        console.log(`Result:`, result);
        return result;
    };
}

class Logger {
    @log
    public logMessage(message: string): void {
        console.log(message);
    }
}

// Generator function
function* numberGenerator(n: number): Generator<number> {
    for (let i = 0; i < n; i++) {
        yield i;
    }
}

// Async generator
async function* asyncDataGenerator(urls: string[]): AsyncGenerator<any> {
    for (const url of urls) {
        const data = await fetchData(url);
        yield data;
    }
}

// Namespace
namespace Geometry {
    export interface Point {
        x: number;
        y: number;
    }
    
    export function distance(p1: Point, p2: Point): number {
        return Math.sqrt((p2.x - p1.x) ** 2 + (p2.y - p1.y) ** 2);
    }
    
    class Circle {
        constructor(public center: Point, public radius: number) {}
        
        getArea(): number {
            return Math.PI * this.radius ** 2;
        }
    }
    
    export { Circle };
}

// Module augmentation (if applicable)
// import { SomeLib } from 'some-lib';
// declare module 'some-lib' {
//     interface SomeLib {
//         newMethod(): void;
//     }
// }

// Function calls and relationships
function demonstrateAll(): void {
    // Function calls
    console.log(greet('TypeScript'));
    console.log(add(10, 5));
    console.log(calculate(10, 5, 'multiply'));
    console.log(sumAll(1, 2, 3, 4, 5));
    
    // Generic function calls
    console.log(identity<string>('hello'));
    console.log(identity<number>(42));
    console.log(pair('first', 42));
    
    // Class instantiation and method calls
    const circle = new Circle('MyCircle', 5);
    circle.draw();
    console.log(circle.getArea());
    console.log(circle.perimeter());
    
    const dog = new Dog('Buddy', 'Golden Retriever');
    dog.makeSound();
    dog.move();
    console.log(dog.getBreed());
    
    // Generic class usage
    const stringBox = new Box<string>('Hello');
    const numberBox = new Box<number>(42);
    console.log(stringBox.getContent());
    console.log(numberBox.getContent());
    
    // Bank account operations
    const account = new BankAccount('John Doe', '12345', 1000);
    account.deposit(500);
    account.withdraw(200);
    console.log(account.getBalance());
    
    // Savings account with inheritance
    const savings = new SavingsAccount('Jane Doe', '67890', 0.05);
    savings.deposit(1000);
    savings.applyInterest();
    console.log(savings.getBalance());
    
    // Temperature with getters/setters
    const temp = new Temperature(25);
    console.log(temp.celsius);
    console.log(temp.fahrenheit);
    temp.fahrenheit = 86;
    console.log(temp.celsius);
    
    // Static methods
    console.log(Utility.getInstanceCount());
    console.log(Utility.isEven(4));
    
    // Enum usage
    console.log(Color.Red);
    console.log(Priority.High);
    console.log(Direction.Up);
    
    // Generator usage
    const gen = numberGenerator(5);
    console.log([...gen]);
    
    // Namespace usage
    const point1: Geometry.Point = { x: 0, y: 0 };
    const point2: Geometry.Point = { x: 3, y: 4 };
    console.log(Geometry.distance(point1, point2));
    
    const geometryCircle = new Geometry.Circle(point1, 5);
    console.log(geometryCircle.getArea());
}

// Export
export {
    Person,
    Drawable,
    Shape,
    Color,
    Priority,
    Direction,
    Employee,
    greet,
    add,
    multiply,
    calculate,
    Circle,
    Dog,
    Box,
    BankAccount,
    SavingsAccount,
    Temperature,
    Utility,
    Geometry
};

export default demonstrateAll;
