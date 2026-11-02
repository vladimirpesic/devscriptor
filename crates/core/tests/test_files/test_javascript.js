/**
 * Comprehensive JavaScript test file containing all entities and relationships.
 */

// Imports (ES6)
import { utilityFunction } from './utils.js';
import * as MathUtils from './math.js';

// Function declarations
function greet(name) {
    return `Hello, ${name}!`;
}

// Function expression
const add = function(a, b) {
    return a + b;
};

// Arrow functions
const subtract = (a, b) => a - b;
const multiply = (a, b) => {
    return a * b;
};

const divide = (a, b) => b !== 0 ? a / b : 0;

// Arrow function with default parameters
const calculate = (a, b, operation = 'add') => {
    switch (operation) {
        case 'add': return a + b;
        case 'subtract': return a - b;
        case 'multiply': return a * b;
        case 'divide': return a / b;
        default: return 0;
    }
};

// Rest parameters
function sumAll(...numbers) {
    return numbers.reduce((acc, num) => acc + num, 0);
}

// Destructuring parameters
function createUser({ name, age, email = 'default@example.com' }) {
    return { name, age, email };
}

// Async function
async function fetchData(url) {
    try {
        const response = await fetch(url);
        const data = await response.json();
        return data;
    } catch (error) {
        console.error('Error:', error);
        return null;
    }
}

// Async function with multiple awaits
async function fetchMultiple(urls) {
    const promises = urls.map(url => fetch(url).then(res => res.json()));
    return Promise.all(promises);
}

// Class declaration
class Person {
    constructor(name, age, email) {
        this.name = name;
        this.age = age;
        this.email = email || 'default@example.com';
    }
    
    greet() {
        return `Hello, I'm ${this.name}`;
    }
    
    getDetails() {
        return `${this.name}, ${this.age} years old`;
    }
    
    static createDefault() {
        return new Person('John Doe', 30);
    }
}

// Class inheritance
class Employee extends Person {
    constructor(name, age, email, position, salary) {
        super(name, age, email);
        this.position = position;
        this.salary = salary;
    }
    
    getDetails() {
        return `${super.getDetails()}, ${this.position}`;
    }
    
    getAnnualSalary() {
        return this.salary * 12;
    }
}

// Multiple inheritance through mixins
const Logger = {
    log(message) {
        console.log(`[LOG] ${new Date().toISOString()}: ${message}`);
    },
    
    error(message) {
        console.error(`[ERROR] ${new Date().toISOString()}: ${message}`);
    }
};

class LoggerPerson extends Person {
    constructor(name, age, email) {
        super(name, age, email);
    }
    
    greet() {
        this.log('Greeting called');
        return super.greet();
    }
}

Object.assign(LoggerPerson.prototype, Logger);

// Interface-like pattern (using JSDoc)
/**
 * @interface Drawable
 */
class Shape {
    constructor(name) {
        this.name = name;
    }
    
    /**
     * @abstract
     */
    area() {
        throw new Error('Method area() must be implemented');
    }
    
    /**
     * @abstract
     */
    perimeter() {
        throw new Error('Method perimeter() must be implemented');
    }
}

class Circle extends Shape {
    constructor(name, radius) {
        super(name);
        this.radius = radius;
    }
    
    area() {
        return Math.PI * this.radius ** 2;
    }
    
    perimeter() {
        return 2 * Math.PI * this.radius;
    }
}

// Class with private fields (#)
class BankAccount {
    #balance;
    
    constructor(initialBalance = 0) {
        this.#balance = initialBalance;
    }
    
    deposit(amount) {
        if (amount > 0) {
            this.#balance += amount;
        }
    }
    
    withdraw(amount) {
        if (amount > 0 && amount <= this.#balance) {
            this.#balance -= amount;
        }
    }
    
    getBalance() {
        return this.#balance;
    }
}

// Class with getters and setters
class Temperature {
    #celsius;
    
    constructor(celsius = 0) {
        this.#celsius = celsius;
    }
    
    get celsius() {
        return this.#celsius;
    }
    
    set celsius(value) {
        this.#celsius = value;
    }
    
    get fahrenheit() {
        return this.#celsius * 9/5 + 32;
    }
    
    set fahrenheit(value) {
        this.#celsius = (value - 32) * 5/9;
    }
}

// Generator function
function* numberGenerator(n) {
    for (let i = 0; i < n; i++) {
        yield i;
    }
}

// Async generator
async function* asyncDataGenerator(urls) {
    for (const url of urls) {
        const data = await fetchData(url);
        yield data;
    }
}

// Higher-order function
function withLogging(fn) {
    return function(...args) {
        console.log(`Calling ${fn.name} with args:`, args);
        const result = fn.apply(this, args);
        console.log(`Result:`, result);
        return result;
    };
}

// Function calls and relationships
function demonstrateAll() {
    // Function calls
    console.log(greet('World'));
    console.log(add(10, 5));
    console.log(subtract(10, 5));
    console.log(multiply(10, 5));
    console.log(divide(10, 5));
    console.log(calculate(10, 5, 'multiply'));
    console.log(sumAll(1, 2, 3, 4, 5));
    
    // Object creation and method calls
    const person = new Person('Alice', 25, 'alice@example.com');
    console.log(person.greet());
    console.log(person.getDetails());
    
    const employee = new Employee('Bob', 30, 'bob@example.com', 'Developer', 5000);
    console.log(employee.getDetails());
    console.log(employee.getAnnualSalary());
    
    // Static method call
    const defaultPerson = Person.createDefault();
    console.log(defaultPerson.greet());
    
    // Shape and Circle
    const circle = new Circle('MyCircle', 5);
    console.log(circle.area());
    console.log(circle.perimeter());
    
    // BankAccount with private fields
    const account = new BankAccount(1000);
    account.deposit(500);
    account.withdraw(200);
    console.log(account.getBalance());
    
    // Temperature with getters/setters
    const temp = new Temperature(25);
    console.log(temp.celsius);
    console.log(temp.fahrenheit);
    temp.fahrenheit = 86;
    console.log(temp.celsius);
    
    // Generator usage
    const gen = numberGenerator(5);
    console.log([...gen]);
    
    // Higher-order function usage
    const loggedAdd = withLogging(add);
    console.log(loggedAdd(10, 20));
    
    // Array methods (callbacks)
    const numbers = [1, 2, 3, 4, 5];
    const doubled = numbers.map(n => n * 2);
    const evens = numbers.filter(n => n % 2 === 0);
    const sum = numbers.reduce((acc, n) => acc + n, 0);
    
    console.log(doubled, evens, sum);
}

// IIFE (Immediately Invoked Function Expression)
(function() {
    console.log('IIFE executed');
})();

// Module pattern
const Module = (function() {
    let privateVar = 'private';
    
    function privateFunction() {
        return 'private result';
    }
    
    return {
        publicMethod: function() {
            return privateFunction();
        },
        getVar: function() {
            return privateVar;
        }
    };
})();

// Export (ES6)
export {
    greet,
    add,
    Person,
    Employee,
    Circle,
    demonstrateAll
};

// Default export
export default function main() {
    demonstrateAll();
}
