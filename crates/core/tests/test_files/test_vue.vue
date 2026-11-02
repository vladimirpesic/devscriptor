<!--
  Comprehensive Vue test file containing all entities and relationships.
-->

<template>
  <div class="container">
    <!-- Component header -->
    <header>
      <h1>{{ title }}</h1>
      <p>{{ description }}</p>
    </header>

    <!-- Counter section -->
    <section class="counter-section">
      <h2>Counter</h2>
      <p>Count: {{ count }}</p>
      <p>Doubled: {{ doubled }}</p>
      <button @click="increment">Increment</button>
      <button @click="decrement">Decrement</button>
      <button @click="reset">Reset</button>
    </section>

    <!-- User info section -->
    <section class="user-section">
      <h2>User Information</h2>
      <div v-if="user">
        <p>Name: {{ user.name }}</p>
        <p>Email: {{ user.email }}</p>
        <p>Age: {{ user.age }}</p>
      </div>
      <div v-else>
        <p>No user loaded</p>
      </div>
      <button @click="loadUser">Load User</button>
    </section>

    <!-- Todo list section -->
    <section class="todo-section">
      <h2>Todo List</h2>
      <input 
        v-model="newTodo" 
        @keyup.enter="addTodo"
        placeholder="Add a new todo"
      />
      <button @click="addTodo">Add</button>
      
      <ul>
        <li 
          v-for="todo in filteredTodos" 
          :key="todo.id"
          :class="{ completed: todo.completed }"
        >
          <input 
            type="checkbox" 
            v-model="todo.completed"
          />
          <span>{{ todo.text }}</span>
          <button @click="removeTodo(todo.id)">Delete</button>
        </li>
      </ul>
      
      <div class="filters">
        <button @click="filter = 'all'">All</button>
        <button @click="filter = 'active'">Active</button>
        <button @click="filter = 'completed'">Completed</button>
      </div>
    </section>

    <!-- Slots example -->
    <Card>
      <template #header>
        <h3>Card Header</h3>
      </template>
      <template #default>
        <p>Card content goes here</p>
      </template>
      <template #footer>
        <button>Card Action</button>
      </template>
    </Card>

    <!-- Component with props and events -->
    <ChildComponent 
      :message="parentMessage"
      :count="count"
      @update="handleChildUpdate"
      @submit="handleChildSubmit"
    />
  </div>
</template>

<script>
// Options API example
export default {
  name: 'TestVue',
  
  // Component registration
  components: {
    Card: () => import('./Card.vue'),
    ChildComponent: () => import('./ChildComponent.vue')
  },
  
  // Props definition
  props: {
    title: {
      type: String,
      default: 'Vue Test Component'
    },
    description: {
      type: String,
      default: 'A comprehensive Vue.js test component'
    },
    initialCount: {
      type: Number,
      default: 0
    }
  },
  
  // Data function
  data() {
    return {
      count: this.initialCount,
      newTodo: '',
      filter: 'all',
      user: null,
      todos: [
        { id: 1, text: 'Learn Vue', completed: true },
        { id: 2, text: 'Build an app', completed: false }
      ],
      parentMessage: 'Hello from parent'
    }
  },
  
  // Computed properties
  computed: {
    doubled() {
      return this.count * 2
    },
    
    filteredTodos() {
      switch (this.filter) {
        case 'active':
          return this.todos.filter(t => !t.completed)
        case 'completed':
          return this.todos.filter(t => t.completed)
        default:
          return this.todos
      }
    },
    
    completedCount() {
      return this.todos.filter(t => t.completed).length
    },
    
    remainingCount() {
      return this.todos.filter(t => !t.completed).length
    }
  },
  
  // Methods
  methods: {
    increment() {
      this.count++
      this.$emit('count-changed', this.count)
    },
    
    decrement() {
      this.count--
    },
    
    reset() {
      this.count = this.initialCount
    },
    
    addTodo() {
      if (this.newTodo.trim()) {
        this.todos.push({
          id: Date.now(),
          text: this.newTodo,
          completed: false
        })
        this.newTodo = ''
      }
    },
    
    removeTodo(id) {
      this.todos = this.todos.filter(t => t.id !== id)
    },
    
    async loadUser() {
      try {
        const response = await fetch('/api/user')
        this.user = await response.json()
      } catch (error) {
        console.error('Failed to load user:', error)
      }
    },
    
    handleChildUpdate(value) {
      console.log('Child updated:', value)
    },
    
    handleChildSubmit(data) {
      console.log('Child submitted:', data)
    }
  },
  
  // Lifecycle hooks
  created() {
    console.log('Component created')
  },
  
  mounted() {
    console.log('Component mounted')
    this.loadUser()
  },
  
  updated() {
    console.log('Component updated')
  },
  
  beforeDestroy() {
    console.log('Component will be destroyed')
  },
  
  // Watchers
  watch: {
    count(newVal, oldVal) {
      console.log(`Count changed from ${oldVal} to ${newVal}`)
    },
    
    todos: {
      deep: true,
      handler(newVal) {
        localStorage.setItem('todos', JSON.stringify(newVal))
      }
    }
  },
  
  // Mixins
  mixins: [LoggerMixin],
  
  // Directives
  directives: {
    focus: {
      inserted(el) {
        el.focus()
      }
    }
  }
}
</script>

<script setup>
// Composition API example (Vue 3)
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useStore } from 'vuex'
import { useRoute, useRouter } from 'vue-router'

// Reactive state
const count = ref(0)
const name = ref('')
const items = ref([])

// Computed properties
const doubled = computed(() => count.value * 2)
const isEven = computed(() => count.value % 2 === 0)

// Methods
function increment() {
  count.value++
}

function addItem(item) {
  items.value.push(item)
}

// Lifecycle hooks
onMounted(() => {
  console.log('Setup: Component mounted')
})

onUnmounted(() => {
  console.log('Setup: Component unmounted')
})

// Watchers
watch(count, (newVal, oldVal) => {
  console.log(`Count changed: ${oldVal} -> ${newVal}`)
})

// Store and router
const store = useStore()
const route = useRoute()
const router = useRouter()

// Async function
async function fetchData() {
  const response = await fetch('/api/data')
  const data = await response.json()
  items.value = data
}
</script>

<style scoped>
/* Scoped styles */
.container {
  max-width: 800px;
  margin: 0 auto;
  padding: 20px;
}

header {
  text-align: center;
  margin-bottom: 30px;
}

section {
  margin-bottom: 30px;
  padding: 20px;
  border: 1px solid #ddd;
  border-radius: 8px;
}

.counter-section button {
  margin: 5px;
  padding: 10px 20px;
  background: #42b883;
  color: white;
  border: none;
  border-radius: 4px;
  cursor: pointer;
}

.todo-section input {
  padding: 8px;
  margin-right: 10px;
  border: 1px solid #ccc;
  border-radius: 4px;
}

.todo-section ul {
  list-style: none;
  padding: 0;
}

.todo-section li {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px;
  border-bottom: 1px solid #eee;
}

.todo-section li.completed span {
  text-decoration: line-through;
  color: #999;
}

.filters button {
  margin: 5px;
  padding: 5px 15px;
}

.filters button.active {
  background: #42b883;
  color: white;
}
</style>

<style>
/* Global styles */
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  margin: 0;
  padding: 0;
}
</style>
