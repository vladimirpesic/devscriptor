/**
 * Comprehensive C test file containing all entities and relationships.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// Preprocessor macros
#define MAX_SIZE 100
#define PI 3.14159
#define SQUARE(x) ((x) * (x))

// Function-like macro
#define MAX(a, b) ((a) > (b) ? (a) : (b))

// Conditional compilation
#ifdef DEBUG
    #define LOG(msg) printf("DEBUG: %s\n", msg)
#else
    #define LOG(msg)
#endif

// Enum
typedef enum {
    RED,
    GREEN,
    BLUE
} Color;

typedef enum {
    LOW = 1,
    MEDIUM = 2,
    HIGH = 3
} Priority;

// Struct
typedef struct {
    char name[50];
    int age;
    char email[100];
} Person;

typedef struct {
    char name[50];
    double radius;
} Circle;

// Function pointer typedef
typedef int (*CompareFunc)(const void*, const void*);

// Global variables
int global_var = 100;

// Function declaration
int add(int a, int b);
int multiply(int a, int b);
void print_person(Person* p);

// Function definition
int add(int a, int b) {
    return a + b;
}

int multiply(int a, int b) {
    return a * b;
}

double calculate_area(double radius) {
    return PI * radius * radius;
}

void print_person(Person* p) {
    printf("Name: %s, Age: %d\n", p->name, p->age);
}

// Function with pointer parameters
void swap(int* a, int* b) {
    int temp = *a;
    *a = *b;
    *b = temp;
}

// Function returning pointer
Person* create_person(const char* name, int age) {
    Person* p = (Person*)malloc(sizeof(Person));
    if (p) {
        strncpy(p->name, name, sizeof(p->name) - 1);
        p->age = age;
    }
    return p;
}

// Callback function
int compare_ints(const void* a, const void* b) {
    return (*(int*)a - *(int*)b);
}

void sort_array(int* arr, int size, CompareFunc compare) {
    qsort(arr, size, sizeof(int), compare);
}

// Nested function (GNU extension)
void demonstrate_nested() {
    int outer = 10;
    
    int inner(int x) {
        return x + outer;
    }
    
    printf("Inner result: %d\n", inner(5));
}

// Main function
int main() {
    // Variable declarations
    int a = 10, b = 20;
    double radius = 5.0;
    
    // Function calls
    printf("Add: %d\n", add(a, b));
    printf("Multiply: %d\n", multiply(a, b));
    printf("Area: %.2f\n", calculate_area(radius));
    printf("Max: %d\n", MAX(a, b));
    
    // Struct usage
    Person p = {"John Doe", 30, "john@example.com"};
    print_person(&p);
    
    Person* p2 = create_person("Jane Doe", 25);
    if (p2) {
        print_person(p2);
        free(p2);
    }
    
    // Pointer usage
    int x = 5, y = 10;
    swap(&x, &y);
    printf("After swap: x=%d, y=%d\n", x, y);
    
    // Array sorting with callback
    int arr[] = {5, 2, 8, 1, 9};
    sort_array(arr, 5, compare_ints);
    
    // Enum usage
    Color c = RED;
    Priority p = HIGH;
    
    return 0;
}
