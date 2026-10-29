// Comprehensive TSX test file

import React, { useState, useEffect, useContext, createContext, useCallback, useMemo, useRef, useImperativeHandle, forwardRef, memo, lazy, Suspense } from 'react';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';

// ===== Type Definitions =====

interface User {
  id: number;
  name: string;
  email: string;
  age: number;
  isActive: boolean;
  role: 'admin' | 'user' | 'guest';
}

interface Todo {
  id: number;
  title: string;
  completed: boolean;
  dueDate?: Date;
  tags: string[];
}

interface Product {
  id: number;
  name: string;
  price: number;
  description: string;
  inStock: boolean;
}

type Theme = 'light' | 'dark' | 'auto';
type Status = 'idle' | 'loading' | 'success' | 'error';

interface AppState {
  user: User | null;
  theme: Theme;
  status: Status;
  error: string | null;
}

// ===== Type Utilities =====

type PartialUser = Partial<User>;
type RequiredUser = Required<User>;
type ReadonlyUser = Readonly<User>;

type TodoKeys = keyof Todo;
type TodoValues = Todo[TodoKeys];

type UserProps = Pick<User, 'name' | 'email'>;
type UserOmitted = Omit<User, 'age' | 'isActive'>;

type EventHandler = (event: React.MouseEvent<HTMLButtonElement>) => void;
type AsyncFunction = <T>(value: T) => Promise<T>;

// ===== Context =====

interface AppContextType {
  state: AppState;
  dispatch: React.Dispatch<AppAction>;
}

const AppContext = createContext<AppContextType | undefined>(undefined);

type AppAction =
  | { type: 'SET_USER'; payload: User }
  | { type: 'CLEAR_USER' }
  | { type: 'SET_THEME'; payload: Theme }
  | { type: 'SET_STATUS'; payload: Status }
  | { type: 'SET_ERROR'; payload: string | null };

// ===== Custom Hooks =====

function useCounter(initialValue: number = 0, step: number = 1) {
  const [count, setCount] = useState<number>(initialValue);

  const increment = useCallback(() => setCount(c => c + step), [step]);
  const decrement = useCallback(() => setCount(c => c - step), [step]);
  const reset = useCallback(() => setCount(initialValue), [initialValue]);

  return { count, increment, decrement, reset };
}

function useLocalStorage<T>(key: string, initialValue: T) {
  const [storedValue, setStoredValue] = useState<T>(() => {
    try {
      const item = window.localStorage.getItem(key);
      return item ? JSON.parse(item) : initialValue;
    } catch (error) {
      return initialValue;
    }
  });

  const setValue = useCallback((value: T | ((val: T) => T)) => {
    try {
      const valueToStore = value instanceof Function ? value(storedValue) : value;
      setStoredValue(valueToStore);
      window.localStorage.setItem(key, JSON.stringify(valueToStore));
    } catch (error) {
      console.error(error);
    }
  }, [key, storedValue]);

  return [storedValue, setValue] as const;
}

function useFetch<T>(url: string) {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();

    const fetchData = async () => {
      try {
        setLoading(true);
        const response = await fetch(url, { signal: controller.signal });
        
        if (!response.ok) {
          throw new Error(`HTTP error! status: ${response.status}`);
        }
        
        const json = await response.json();
        setData(json);
        setError(null);
      } catch (err) {
        if (err instanceof Error && err.name !== 'AbortError') {
          setError(err.message);
        }
      } finally {
        setLoading(false);
      }
    };

    fetchData();

    return () => controller.abort();
  }, [url]);

  return { data, loading, error };
}

function useDebounce<T>(value: T, delay: number): T {
  const [debouncedValue, setDebouncedValue] = useState<T>(value);

  useEffect(() => {
    const handler = setTimeout(() => {
      setDebouncedValue(value);
    }, delay);

    return () => {
      clearTimeout(handler);
    };
  }, [value, delay]);

  return debouncedValue;
}

function usePrevious<T>(value: T): T | undefined {
  const ref = useRef<T>();
  useEffect(() => {
    ref.current = value;
  }, [value]);
  return ref.current;
}

// ===== Components =====

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'danger' | 'ghost';
  size?: 'sm' | 'md' | 'lg';
  loading?: boolean;
  fullWidth?: boolean;
}

const Button: React.FC<ButtonProps> = memo(({
  variant = 'primary',
  size = 'md',
  loading = false,
  fullWidth = false,
  children,
  disabled,
  ...props
}) => {
  const baseClasses = 'font-medium rounded-lg transition-colors';
  
  const variantClasses = {
    primary: 'bg-blue-500 text-white hover:bg-blue-600',
    secondary: 'bg-gray-500 text-white hover:bg-gray-600',
    danger: 'bg-red-500 text-white hover:bg-red-600',
    ghost: 'bg-transparent text-gray-700 hover:bg-gray-100',
  };

  const sizeClasses = {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-base',
    lg: 'px-6 py-3 text-lg',
  };

  const widthClass = fullWidth ? 'w-full' : '';

  return (
    <button
      className={`${baseClasses} ${variantClasses[variant]} ${sizeClasses[size]} ${widthClass}`}
      disabled={disabled || loading}
      {...props}
    >
      {loading ? (
        <span className="inline-block animate-spin">⏳</span>
      ) : (
        children
      )}
    </button>
  );
});

Button.displayName = 'Button';

// ===== Input Component =====

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
  helperText?: string;
}

const Input: React.FC<InputProps> = memo(({
  label,
  error,
  helperText,
  className = '',
  ...props
}) => {
  const inputId = `input-${Math.random().toString(36).substr(2, 9)}`;

  return (
    <div className="mb-4">
      {label && (
        <label htmlFor={inputId} className="block text-sm font-medium text-gray-700 mb-1">
          {label}
        </label>
      )}
      <input
        id={inputId}
        className={`w-full px-3 py-2 border rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 ${
          error ? 'border-red-500' : 'border-gray-300'
        } ${className}`}
        {...props}
      />
      {error && (
        <p className="mt-1 text-sm text-red-600">{error}</p>
      )}
      {helperText && !error && (
        <p className="mt-1 text-sm text-gray-500">{helperText}</p>
      )}
    </div>
  );
});

Input.displayName = 'Input';

// ===== Select Component =====

interface SelectProps extends React.SelectHTMLAttributes<HTMLSelectElement> {
  label?: string;
  options: { value: string; label: string }[];
  error?: string;
}

const Select: React.FC<SelectProps> = memo(({
  label,
  options,
  error,
  className = '',
  ...props
}) => {
  const selectId = `select-${Math.random().toString(36).substr(2, 9)}`;

  return (
    <div className="mb-4">
      {label && (
        <label htmlFor={selectId} className="block text-sm font-medium text-gray-700 mb-1">
          {label}
        </label>
      )}
      <select
        id={selectId}
        className={`w-full px-3 py-2 border rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 ${
          error ? 'border-red-500' : 'border-gray-300'
        } ${className}`}
        {...props}
      >
        {options.map(option => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {error && (
        <p className="mt-1 text-sm text-red-600">{error}</p>
      )}
    </div>
  );
});

Select.displayName = 'Select';

// ===== Modal Component =====

interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  children: React.ReactNode;
}

const Modal: React.FC<ModalProps> = ({ isOpen, onClose, title, children }) => {
  useEffect(() => {
    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };

    if (isOpen) {
      document.addEventListener('keydown', handleEscape);
      document.body.style.overflow = 'hidden';
    }

    return () => {
      document.removeEventListener('keydown', handleEscape);
      document.body.style.overflow = 'unset';
    };
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black bg-opacity-50" onClick={onClose} />
      <div className="relative bg-white rounded-lg shadow-xl max-w-md w-full mx-4 p-6">
        {title && (
          <h2 className="text-xl font-semibold mb-4">{title}</h2>
        )}
        {children}
        <button
          onClick={onClose}
          className="absolute top-4 right-4 text-gray-500 hover:text-gray-700"
          aria-label="Close"
        >
          ✕
        </button>
      </div>
    </div>
  );
};

// ===== Card Component =====

interface CardProps {
  children: React.ReactNode;
  className?: string;
  onClick?: () => void;
  interactive?: boolean;
}

const Card: React.FC<CardProps> = ({ children, className = '', onClick, interactive = false }) => {
  const baseClasses = 'bg-white border border-gray-200 rounded-lg shadow-sm p-6';
  const interactiveClasses = interactive 
    ? 'cursor-pointer hover:shadow-md transition-shadow' 
    : '';

  return (
    <div 
      className={`${baseClasses} ${interactiveClasses} ${className}`}
      onClick={onClick}
    >
      {children}
    </div>
  );
};

// ===== Todo Item Component =====

interface TodoItemProps {
  todo: Todo;
  onToggle: (id: number) => void;
  onDelete: (id: number) => void;
}

const TodoItem: React.FC<TodoItemProps> = memo(({ todo, onToggle, onDelete }) => {
  return (
    <Card interactive>
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-3">
          <input
            type="checkbox"
            checked={todo.completed}
            onChange={() => onToggle(todo.id)}
            className="w-5 h-5 text-blue-500 rounded"
          />
          <div>
            <p className={`font-medium ${todo.completed ? 'line-through text-gray-500' : ''}`}>
              {todo.title}
            </p>
            {todo.tags.length > 0 && (
              <div className="flex space-x-2 mt-1">
                {todo.tags.map(tag => (
                  <span key={tag} className="text-xs bg-blue-100 text-blue-800 px-2 py-1 rounded">
                    {tag}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
        <Button
          variant="danger"
          size="sm"
          onClick={() => onDelete(todo.id)}
        >
          Delete
        </Button>
      </div>
    </Card>
  );
});

TodoItem.displayName = 'TodoItem';

// ===== Todo List Component =====

interface TodoListProps {
  todos: Todo[];
  onToggle: (id: number) => void;
  onDelete: (id: number) => void;
}

const TodoList: React.FC<TodoListProps> = ({ todos, onToggle, onDelete }) => {
  if (todos.length === 0) {
    return (
      <div className="text-center py-8 text-gray-500">
        No todos yet. Add one above!
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {todos.map(todo => (
        <TodoItem
          key={todo.id}
          todo={todo}
          onToggle={onToggle}
          onDelete={onDelete}
        />
      ))}
    </div>
  );
};

// ===== User Profile Component =====

interface UserProfileProps {
  user: User;
  onEdit: (user: User) => void;
}

const UserProfile: React.FC<UserProfileProps> = ({ user, onEdit }) => {
  const [isEditing, setIsEditing] = useState(false);
  const [editedUser, setEditedUser] = useState<User>(user);

  const handleSave = useCallback(() => {
    onEdit(editedUser);
    setIsEditing(false);
  }, [editedUser, onEdit]);

  const handleCancel = useCallback(() => {
    setEditedUser(user);
    setIsEditing(false);
  }, [user]);

  return (
    <Card>
      <h2 className="text-2xl font-bold mb-4">User Profile</h2>
      {isEditing ? (
        <div className="space-y-4">
          <Input
            label="Name"
            value={editedUser.name}
            onChange={e => setEditedUser({ ...editedUser, name: e.target.value })}
          />
          <Input
            label="Email"
            type="email"
            value={editedUser.email}
            onChange={e => setEditedUser({ ...editedUser, email: e.target.value })}
          />
          <Input
            label="Age"
            type="number"
            value={editedUser.age}
            onChange={e => setEditedUser({ ...editedUser, age: parseInt(e.target.value) })}
          />
          <Select
            label="Role"
            options={[
              { value: 'admin', label: 'Admin' },
              { value: 'user', label: 'User' },
              { value: 'guest', label: 'Guest' },
            ]}
            value={editedUser.role}
            onChange={e => setEditedUser({ ...editedUser, role: e.target.value as User['role'] })}
          />
          <div className="flex space-x-2">
            <Button onClick={handleSave}>Save</Button>
            <Button variant="secondary" onClick={handleCancel}>Cancel</Button>
          </div>
        </div>
      ) : (
        <div>
          <p className="mb-2"><strong>ID:</strong> {user.id}</p>
          <p className="mb-2"><strong>Name:</strong> {user.name}</p>
          <p className="mb-2"><strong>Email:</strong> {user.email}</p>
          <p className="mb-2"><strong>Age:</strong> {user.age}</p>
          <p className="mb-2"><strong>Role:</strong> {user.role}</p>
          <p className="mb-4"><strong>Status:</strong> {user.isActive ? 'Active' : 'Inactive'}</p>
          <Button onClick={() => setIsEditing(true)}>Edit Profile</Button>
        </div>
      )}
    </Card>
  );
};

// ===== Product List Component =====

interface ProductListProps {
  products: Product[];
  onAddToCart: (product: Product) => void;
}

const ProductList: React.FC<ProductListProps> = ({ products, onAddToCart }) => {
  const [filter, setFilter] = useState('');

  const filteredProducts = useMemo(() => {
    return products.filter(product =>
      product.name.toLowerCase().includes(filter.toLowerCase()) ||
      product.description.toLowerCase().includes(filter.toLowerCase())
    );
  }, [products, filter]);

  return (
    <div>
      <Input
        placeholder="Search products..."
        value={filter}
        onChange={e => setFilter(e.target.value)}
        className="mb-4"
      />
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {filteredProducts.map(product => (
          <Card key={product.id} interactive>
            <h3 className="text-lg font-semibold mb-2">{product.name}</h3>
            <p className="text-gray-600 mb-2">{product.description}</p>
            <p className="text-2xl font-bold mb-4">${product.price.toFixed(2)}</p>
            <Button
              onClick={() => onAddToCart(product)}
              disabled={!product.inStock}
              fullWidth
            >
              {product.inStock ? 'Add to Cart' : 'Out of Stock'}
            </Button>
          </Card>
        ))}
      </div>
    </div>
  );
};

// ===== Main App Component =====

const App: React.FC = () => {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [user, setUser] = useState<User | null>(null);
  const [theme, setTheme] = useLocalStorage<Theme>('theme', 'light');
  const [showModal, setShowModal] = useState(false);

  const [newTodoTitle, setNewTodoTitle] = useState('');
  const [newTodoTags, setNewTodoTags] = useState('');

  const { count: todoCount } = useCounter(todos.length);
  const debouncedSearch = useDebounce(newTodoTitle, 300);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme(prev => prev === 'light' ? 'dark' : 'light');
  }, [setTheme]);

  const addTodo = useCallback(() => {
    if (!newTodoTitle.trim()) return;

    const newTodo: Todo = {
      id: Date.now(),
      title: newTodoTitle,
      completed: false,
      tags: newTodoTags.split(',').map(tag => tag.trim()).filter(Boolean),
    };

    setTodos(prev => [newTodo, ...prev]);
    setNewTodoTitle('');
    setNewTodoTags('');
  }, [newTodoTitle, newTodoTags]);

  const toggleTodo = useCallback((id: number) => {
    setTodos(prev =>
      prev.map(todo =>
        todo.id === id ? { ...todo, completed: !todo.completed } : todo
      )
    );
  }, []);

  const deleteTodo = useCallback((id: number) => {
    setTodos(prev => prev.filter(todo => todo.id !== id));
  }, []);

  const activeTodos = useMemo(() => todos.filter(t => !t.completed), [todos]);
  const completedTodos = useMemo(() => todos.filter(t => t.completed), [todos]);

  const sampleUser: User = {
    id: 1,
    name: 'John Doe',
    email: 'john@example.com',
    age: 30,
    isActive: true,
    role: 'user',
  };

  const sampleProducts: Product[] = [
    {
      id: 1,
      name: 'Premium Widget',
      price: 29.99,
      description: 'A high-quality widget for all your needs',
      inStock: true,
    },
    {
      id: 2,
      name: 'Super Gadget',
      price: 49.99,
      description: 'The latest gadget with advanced features',
      inStock: true,
    },
    {
      id: 3,
      name: 'Ultimate Tool',
      price: 19.99,
      description: 'An essential tool for every household',
      inStock: false,
    },
  ];

  return (
    <div className="min-h-screen bg-gray-50 p-8">
      <div className="max-w-4xl mx-auto">
        <header className="mb-8">
          <h1 className="text-4xl font-bold mb-4">TSX Test Application</h1>
          <div className="flex justify-between items-center">
            <p className="text-gray-600">Total Todos: {todoCount}</p>
            <Button variant="ghost" onClick={toggleTheme}>
              {theme === 'light' ? '🌙 Dark Mode' : '☀️ Light Mode'}
            </Button>
          </div>
        </header>

        <section className="mb-8">
          <h2 className="text-2xl font-bold mb-4">Add New Todo</h2>
          <div className="flex space-x-2 mb-4">
            <Input
              placeholder="Enter todo title..."
              value={newTodoTitle}
              onChange={e => setNewTodoTitle(e.target.value)}
              onKeyPress={e => e.key === 'Enter' && addTodo()}
              className="flex-1"
            />
            <Input
              placeholder="Tags (comma-separated)"
              value={newTodoTags}
              onChange={e => setNewTodoTags(e.target.value)}
              className="flex-1"
            />
          </div>
          <Button onClick={addTodo}>Add Todo</Button>
        </section>

        <section className="mb-8">
          <h2 className="text-2xl font-bold mb-4">Todo List</h2>
          <div className="mb-4">
            <p className="text-sm text-gray-600">
              Active: {activeTodos.length} | Completed: {completedTodos.length}
            </p>
          </div>
          <TodoList
            todos={todos}
            onToggle={toggleTodo}
            onDelete={deleteTodo}
          />
        </section>

        <section className="mb-8">
          <div className="flex justify-between items-center mb-4">
            <h2 className="text-2xl font-bold">User Profile</h2>
            <Button onClick={() => setShowModal(true)}>Show User</Button>
          </div>
        </section>

        <Modal isOpen={showModal} onClose={() => setShowModal(false)}>
          <UserProfile
            user={sampleUser}
            onEdit={setUser}
          />
        </Modal>
      </div>
    </div>
  );
};

export default App;
export { useCounter, useLocalStorage, useFetch, useDebounce, Button, Input, Select, Card, Modal };
