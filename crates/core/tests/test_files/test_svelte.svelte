<!--
  Comprehensive Svelte test file containing all entities and relationships.
-->

<script context="module">
  // Module-level code (runs once per module)
  export const preload = async function() {
    return {
      data: await fetch('/api/data').then(r => r.json())
    };
  };

  // Export constants
  export const APP_NAME = 'TestApp';
  export const VERSION = '1.0.0';
</script>

<script>
  // ============================================================================
  // Imports
  // ============================================================================
  import { onMount, onDestroy, beforeUpdate, afterUpdate, tick } from 'svelte';
  import { writable, readable, derived } from 'svelte/store';
  import { fade, fly, slide, scale } from 'svelte/transition';
  import { flip } from 'svelte/animate';
  import { spring, tweened } from 'svelte/motion';
  import ComponentA from './ComponentA.svelte';
  import ComponentB from './ComponentB.svelte';
  import { userStore } from '../stores/user.js';
  import { formatDate, formatCurrency } from '../utils/helpers.js';

  // ============================================================================
  // Props (Component Properties)
  // ============================================================================
  export let title = 'Default Title';
  export let items = [];
  export let user = null;
  export let config = {};
  export let onSubmit = () => {};
  export let onCancel = () => {};

  // Props with types and defaults (using runes - Svelte 5)
  let {
    initialCount = 0,
    maxItems = 10,
    enableAnimations = true,
    theme = 'light'
  } = $props();

  // ============================================================================
  // State (Reactive Variables)
  // ============================================================================
  // Basic reactive state
  let count = $state(initialCount);
  let name = $state('');
  let email = $state('');
  let isLoading = $state(false);
  let error = $state(null);

  // Derived state
  let doubled = $derived(count * 2);
  let isValid = $derived(name.length > 0 && email.includes('@'));
  let sortedItems = $derived(items.slice().sort((a, b) => a - b));

  // Deep reactive state
  let formData = $state({
    username: '',
    password: '',
    confirmPassword: ''
  });

  // Array state
  let todos = $state([
    { id: 1, text: 'Learn Svelte', done: true },
    { id: 2, text: 'Build an app', done: false }
  ]);

  // ============================================================================
  // Stores
  // ============================================================================
  // Writable store
  const countStore = writable(0);

  // Readable store
  const timeStore = readable(new Date(), (set) => {
    const interval = setInterval(() => {
      set(new Date());
    }, 1000);

    return () => clearInterval(interval);
  });

  // Derived store
  const doubledStore = derived(countStore, $count => $count * 2);

  // Custom store
  function createCounter(initial = 0) {
    const { subscribe, set, update } = writable(initial);

    return {
      subscribe,
      increment: () => update(n => n + 1),
      decrement: () => update(n => n - 1),
      reset: () => set(initial)
    };
  }

  const counter = createCounter(0);

  // ============================================================================
  // Motion/Animation
  // ============================================================================
  // Tweened value
  const progress = tweened(0, {
    duration: 400,
    easing: t => t * t
  });

  // Spring physics
  const coords = spring({ x: 0, y: 0 }, {
    stiffness: 0.1,
    damping: 0.25
  });

  // ============================================================================
  // Lifecycle Functions
  // ============================================================================
  onMount(() => {
    console.log('Component mounted');
    
    // Initialize data
    loadData();

    return () => {
      console.log('Cleanup on unmount');
    };
  });

  onDestroy(() => {
    console.log('Component destroyed');
  });

  beforeUpdate(() => {
    console.log('Before update');
  });

  afterUpdate(() => {
    console.log('After update');
  });

  // ============================================================================
  // Event Handlers
  // ============================================================================
  function handleClick(event) {
    count += 1;
    progress.set(count / 10);
  }

  function handleInput(event) {
    name = event.target.value;
  }

  function handleSubmit(event) {
    event.preventDefault();
    
    if (!isValid) {
      error = 'Please fill in all required fields';
      return;
    }

    isLoading = true;
    error = null;

    // Simulate API call
    setTimeout(() => {
      isLoading = false;
      onSubmit({ name, email });
    }, 1000);
  }

  function handleKeyDown(event) {
    if (event.key === 'Enter') {
      handleSubmit(event);
    }
  }

  function handleMouseMove(event) {
    coords.set({ x: event.clientX, y: event.clientY });
  }

  // ============================================================================
  // Component Logic Functions
  // ============================================================================
  async function loadData() {
    try {
      isLoading = true;
      const response = await fetch('/api/data');
      const data = await response.json();
      items = data.items;
    } catch (err) {
      error = err.message;
    } finally {
      isLoading = false;
    }
  }

  function addTodo(text) {
    if (text.trim()) {
      todos = [...todos, {
        id: Date.now(),
        text,
        done: false
      }];
    }
  }

  function toggleTodo(id) {
    todos = todos.map(todo =>
      todo.id === id ? { ...todo, done: !todo.done } : todo
    );
  }

  function removeTodo(id) {
    todos = todos.filter(todo => todo.id !== id);
  }

  function clearCompleted() {
    todos = todos.filter(todo => !todo.done);
  }

  // ============================================================================
  // Reactive Statements
  // ============================================================================
  // Runs whenever count changes
  $: console.log('Count changed:', count);

  // Reactive declaration
  $: total = items.reduce((sum, item) => sum + item.price, 0);

  // Reactive if statement
  $: if (count > 10) {
    console.log('Count is getting high!');
  }

  // Reactive block
  $: {
    console.log('Multiple dependencies changed');
    console.log('Count:', count);
    console.log('Name:', name);
  }

  // ============================================================================
  // Computed Values
  // ============================================================================
  $: remainingTodos = todos.filter(t => !t.done).length;
  $: completedTodos = todos.filter(t => t.done).length;
  $: progressPercent = (completedTodos / todos.length) * 100 || 0;
  $: allDone = remainingTodos === 0 && todos.length > 0;

  // ============================================================================
  // Bindings
  // ============================================================================
  let textInput;
  let canvas;

  // Bind this element
  function bindElement(element) {
    textInput = element;
  }

  // ============================================================================
  // Context (for child components)
  // ============================================================================
  import { setContext, getContext } from 'svelte';
  
  setContext('appConfig', {
    theme,
    locale: 'en-US',
    currency: 'USD'
  });

  // ============================================================================
  // Directives
  // ============================================================================
  let items2 = [1, 2, 3, 4, 5];
  
  function addItem() {
    items2 = [...items2, items2.length + 1];
  }

  function removeItem(item) {
    items2 = items2.filter(i => i !== item);
  }
</script>

<!-- ============================================================================
     Template / Markup
     ============================================================================ -->

<div class="container" class:loading={isLoading} class:dark={theme === 'dark'}>
  <header>
    <h1>{title}</h1>
    <p>Version: {VERSION}</p>
  </header>

  <!-- Reactive Display -->
  <section class="counter">
    <h2>Counter</h2>
    <p>Count: {count}</p>
    <p>Doubled: {doubled}</p>
    <button on:click={handleClick}>
      Clicked {count} {count === 1 ? 'time' : 'times'}
    </button>
    
    <!-- Progress bar with tweened value -->
    <div class="progress-bar">
      <div class="progress" style="width: {$progress * 100}%"></div>
    </div>
  </section>

  <!-- Form with Two-Way Binding -->
  <section class="form-section">
    <h2>User Form</h2>
    <form on:submit|preventDefault={handleSubmit}>
      <label>
        Name:
        <input
          bind:value={name}
          type="text"
          placeholder="Enter your name"
          on:keydown={handleKeyDown}
        />
      </label>

      <label>
        Email:
        <input
          bind:value={email}
          type="email"
          placeholder="Enter your email"
        />
      </label>

      <button type="submit" disabled={!isValid || isLoading}>
        {#if isLoading}
          Loading...
        {:else}
          Submit
        {/if}
      </button>
    </form>

    {#if error}
      <p class="error" transition:fade>{error}</p>
    {/if}
  </section>

  <!-- Conditional Rendering -->
  <section class="conditional">
    <h2>Conditional Rendering</h2>
    
    {#if count < 5}
      <p>Count is less than 5</p>
    {:else if count < 10}
      <p>Count is between 5 and 10</p>
    {:else}
      <p>Count is 10 or more</p>
    {/if}

    {#await loadData()}
      <p>Loading...</p>
    {:then data}
      <p>Loaded: {data}</p>
    {:catch error}
      <p>Error: {error.message}</p>
    {/await}
  </section>

  <!-- List Rendering with Transitions -->
  <section class="todo-list">
    <h2>Todo List</h2>
    
    <div class="stats">
      <span>{remainingTodos} remaining</span>
      <span>{completedTodos} completed</span>
      {#if allDone}
        <span class="celebration">🎉 All done!</span>
      {/if}
    </div>

    <ul>
      {#each todos as todo (todo.id)}
        <li
          class:completed={todo.done}
          animate:flip
          transition:slide
        >
          <input
            type="checkbox"
            checked={todo.done}
            on:change={() => toggleTodo(todo.id)}
          />
          <span>{todo.text}</span>
          <button on:click={() => removeTodo(todo.id)}>×</button>
        </li>
      {:else}
        <li>No todos yet!</li>
      {/each}
    </ul>

    <button on:click={clearCompleted} disabled={completedTodos === 0}>
      Clear Completed
    </button>
  </section>

  <!-- Store Subscriptions -->
  <section class="stores">
    <h2>Store Values</h2>
    <p>Count Store: {$countStore}</p>
    <p>Doubled Store: {$doubledStore}</p>
    <p>Counter: {$counter}</p>
    <p>Current Time: {$timeStore.toLocaleTimeString()}</p>
    
    <button on:click={() => countStore.update(n => n + 1)}>
      Increment Store
    </button>
    <button on:click={counter.increment}>
      Increment Counter
    </button>
  </section>

  <!-- Component Composition -->
  <section class="components">
    <h2>Child Components</h2>
    
    <ComponentA prop1={name} prop2={count} />
    
    <ComponentB let:data>
      <p>Received from slot: {data}</p>
    </ComponentB>
  </section>

  <!-- Event Modifiers -->
  <section class="events">
    <h2>Event Handling</h2>
    
    <button on:click|once={() => alert('Once!')}>
      Click Once
    </button>
    
    <button on:click|preventDefault={() => console.log('Prevented')}>
      Prevent Default
    </button>
    
    <button on:click|stopPropagation={() => console.log('Stopped')}>
      Stop Propagation
    </button>
  </section>

  <!-- Spring Physics -->
  <section class="motion" on:mousemove={handleMouseMove}>
    <h2>Spring Motion</h2>
    <div
      class="spring-box"
      style="transform: translate({$coords.x}px, {$coords.y}px)"
    >
      Follow the mouse!
    </div>
  </section>
</div>

<!-- ============================================================================
     Styles
     ============================================================================ -->

<style>
  :global(body) {
    font-family: system-ui, sans-serif;
    margin: 0;
    padding: 0;
  }

  .container {
    max-width: 800px;
    margin: 0 auto;
    padding: 20px;
  }

  .container.loading {
    opacity: 0.5;
    pointer-events: none;
  }

  .container.dark {
    background: #1a1a1a;
    color: #fff;
  }

  header {
    border-bottom: 2px solid #ddd;
    margin-bottom: 20px;
    padding-bottom: 10px;
  }

  h1 {
    margin: 0;
    color: #ff3e00;
  }

  section {
    margin-bottom: 30px;
    padding: 20px;
    border: 1px solid #ddd;
    border-radius: 8px;
  }

  .counter button {
    background: #ff3e00;
    color: white;
    border: none;
    padding: 10px 20px;
    border-radius: 4px;
    cursor: pointer;
  }

  .progress-bar {
    width: 100%;
    height: 20px;
    background: #eee;
    border-radius: 10px;
    overflow: hidden;
    margin-top: 10px;
  }

  .progress {
    height: 100%;
    background: #ff3e00;
    transition: width 0.3s ease;
  }

  .form-section input {
    display: block;
    width: 100%;
    padding: 8px;
    margin: 5px 0;
    border: 1px solid #ccc;
    border-radius: 4px;
  }

  .error {
    color: #d32f2f;
    background: #ffebee;
    padding: 10px;
    border-radius: 4px;
  }

  .todo-list li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px;
    border-bottom: 1px solid #eee;
  }

  .todo-list li.completed span {
    text-decoration: line-through;
    opacity: 0.5;
  }

  .celebration {
    color: #4caf50;
    font-weight: bold;
  }

  .spring-box {
    width: 100px;
    height: 100px;
    background: #ff3e00;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: white;
    font-size: 12px;
    text-align: center;
  }

  @media (max-width: 600px) {
    .container {
      padding: 10px;
    }
  }
</style>
