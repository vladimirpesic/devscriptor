/**
 * Comprehensive CUDA test file containing all entities and relationships.
 */

// Note: CUDA headers omitted for parsing test - tree-sitter parses syntax only
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

// Constants
#define BLOCK_SIZE 256
#define GRID_SIZE 128
#define N 1024

// Error checking macro
#define cudaCheckError(ans) { gpuAssert((ans), __FILE__, __LINE__); }
inline void gpuAssert(cudaError_t code, const char *file, int line, bool abort=true) {
    if (code != cudaSuccess) {
        fprintf(stderr, "GPUassert: %s %s %d\n", cudaGetErrorString(code), file, line);
        if (abort) exit(code);
    }
}

// ============================================================================
// Kernels
// ============================================================================

// Simple kernel
__global__ void simpleKernel(float *data, int n) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        data[idx] = data[idx] * 2.0f;
    }
}

// Kernel with shared memory
__global__ void sharedMemoryKernel(float *input, float *output, int n) {
    __shared__ float sharedData[BLOCK_SIZE];

    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    int tid = threadIdx.x;

    if (idx < n) {
        sharedData[tid] = input[idx];
    }
    __syncthreads();

    // Perform reduction in shared memory
    for (int stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (tid < stride) {
            sharedData[tid] += sharedData[tid + stride];
        }
        __syncthreads();
    }

    if (tid == 0) {
        output[blockIdx.x] = sharedData[0];
    }
}

// Matrix multiplication kernel
__global__ void matrixMultiplyKernel(float *A, float *B, float *C, int width) {
    int row = blockIdx.y * blockDim.y + threadIdx.y;
    int col = blockIdx.x * blockDim.x + threadIdx.x;

    if (row < width && col < width) {
        float sum = 0.0f;
        for (int k = 0; k < width; k++) {
            sum += A[row * width + k] * B[k * width + col];
        }
        C[row * width + col] = sum;
    }
}

// Atomic operations kernel
__global__ void atomicKernel(int *counter, int n) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        atomicAdd(counter, 1);
    }
}

// Template kernel
__global__ void vectorAddKernel(float *a, float *b, float *c, int n) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        c[idx] = a[idx] + b[idx];
    }
}

// Texture memory kernel (using newer CUDA features)
__global__ void textureKernel(float *output, int width, int height) {
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;

    if (x < width && y < height) {
        int idx = y * width + x;
        output[idx] = sqrtf((float)(x * x + y * y));
    }
}

// Cooperative groups kernel (modern CUDA)
__global__ void cooperativeKernel(float *data, int n) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        data[idx] = sqrtf(data[idx]);
    }
}

// ============================================================================
// Device Functions
// ============================================================================

// Simple device function
__device__ float deviceAdd(float a, float b) {
    return a + b;
}

// Device function with math operations
__device__ float deviceNormalize(float x, float min, float max) {
    return (x - min) / (max - min);
}

// Device function using intrinsics
__device__ float deviceFastSqrt(float x) {
    return __fsqrt_rn(x);
}

// Inline device function
__device__ __forceinline__ float inlineMax(float a, float b) {
    return (a > b) ? a : b;
}

// Template device function
__device__ float deviceClamp(float value, float min, float max) {
    if (value < min) return min;
    if (value > max) return max;
    return value;
}

// ============================================================================
// Host Functions
// ============================================================================

// Memory allocation wrapper
float* allocateDeviceMemory(size_t size) {
    float *d_ptr;
    cudaCheckError(cudaMalloc(&d_ptr, size));
    return d_ptr;
}

// Memory copy wrapper
void copyToDevice(void *dst, const void *src, size_t size) {
    cudaCheckError(cudaMemcpy(dst, src, size, cudaMemcpyHostToDevice));
}

void copyToHost(void *dst, const void *src, size_t size) {
    cudaCheckError(cudaMemcpy(dst, src, size, cudaMemcpyDeviceToHost));
}

// Initialization function
void initializeArray(float *arr, int n, float value) {
    for (int i = 0; i < n; i++) {
        arr[i] = value;
    }
}

// Verification function
bool verifyResults(float *data, int n, float expected) {
    for (int i = 0; i < n; i++) {
        if (fabs(data[i] - expected) > 1e-5) {
            return false;
        }
    }
    return true;
}

// Performance measurement
float measureKernelTime(void (*kernel)(float*, int), float *d_data, int n,
                       dim3 gridSize, dim3 blockSize) {
    cudaEvent_t start, stop;
    cudaCheckError(cudaEventCreate(&start));
    cudaCheckError(cudaEventCreate(&stop));

    cudaCheckError(cudaEventRecord(start));
    simpleKernel<<<gridSize, blockSize>>>(d_data, n);
    cudaCheckError(cudaEventRecord(stop));
    cudaCheckError(cudaEventSynchronize(stop));

    float milliseconds = 0;
    cudaCheckError(cudaEventElapsedTime(&milliseconds, start, stop));

    cudaCheckError(cudaEventDestroy(start));
    cudaCheckError(cudaEventDestroy(stop));

    return milliseconds;
}

// ============================================================================
// Classes and Structs
// ============================================================================

// CUDA stream wrapper class
class CudaStream {
private:
    cudaStream_t stream;
    bool owned;

public:
    CudaStream() : owned(true) {
        cudaCheckError(cudaStreamCreate(&stream));
    }

    explicit CudaStream(cudaStream_t existing) : stream(existing), owned(false) {}

    ~CudaStream() {
        if (owned) {
            cudaStreamDestroy(stream);
        }
    }

    // Disable copy
    CudaStream(const CudaStream&) = delete;
    CudaStream& operator=(const CudaStream&) = delete;

    // Enable move
    CudaStream(CudaStream&& other) noexcept : stream(other.stream), owned(other.owned) {
        other.owned = false;
    }

    cudaStream_t get() const { return stream; }

    void synchronize() const {
        cudaCheckError(cudaStreamSynchronize(stream));
    }
};

// Device memory wrapper template
template<typename T>
class DeviceBuffer {
private:
    T *d_ptr;
    size_t size;

public:
    explicit DeviceBuffer(size_t n) : size(n) {
        cudaCheckError(cudaMalloc(&d_ptr, n * sizeof(T)));
    }

    ~DeviceBuffer() {
        cudaFree(d_ptr);
    }

    // Disable copy
    DeviceBuffer(const DeviceBuffer&) = delete;
    DeviceBuffer& operator=(const DeviceBuffer&) = delete;

    // Enable move
    DeviceBuffer(DeviceBuffer&& other) noexcept
        : d_ptr(other.d_ptr), size(other.size) {
        other.d_ptr = nullptr;
        other.size = 0;
    }

    T* get() const { return d_ptr; }
    size_t getSize() const { return size; }

    void upload(const T *hostData) {
        cudaCheckError(cudaMemcpy(d_ptr, hostData, size * sizeof(T),
                                 cudaMemcpyHostToDevice));
    }

    void download(T *hostData) const {
        cudaCheckError(cudaMemcpy(hostData, d_ptr, size * sizeof(T),
                                 cudaMemcpyDeviceToHost));
    }

    void memset(int value) {
        cudaCheckError(cudaMemset(d_ptr, value, size * sizeof(T)));
    }
};

// GPU device information class
class GpuDevice {
private:
    int deviceId;
    cudaDeviceProp properties;

public:
    explicit GpuDevice(int id = 0) : deviceId(id) {
        cudaCheckError(cudaGetDeviceProperties(&properties, id));
    }

    void setDevice() const {
        cudaCheckError(cudaSetDevice(deviceId));
    }

    void printInfo() const {
        printf("Device %d: %s\n", deviceId, properties.name);
        printf("  Compute Capability: %d.%d\n", properties.major, properties.minor);
        printf("  Total Global Memory: %zu MB\n", properties.totalGlobalMem / (1024 * 1024));
        printf("  Multiprocessors: %d\n", properties.multiProcessorCount);
        printf("  Max Threads per Block: %d\n", properties.maxThreadsPerBlock);
    }

    int getId() const { return deviceId; }
    const cudaDeviceProp& getProperties() const { return properties; }
};

// Vector operation class
class VectorOperations {
public:
    // Add two vectors
    static void add(const float *a, const float *b, float *c, int n) {
        float *d_a, *d_b, *d_c;
        size_t size = n * sizeof(float);

        cudaMalloc(&d_a, size);
        cudaMalloc(&d_b, size);
        cudaMalloc(&d_c, size);

        cudaMemcpy(d_a, a, size, cudaMemcpyHostToDevice);
        cudaMemcpy(d_b, b, size, cudaMemcpyHostToDevice);

        int threadsPerBlock = BLOCK_SIZE;
        int blocksPerGrid = (n + threadsPerBlock - 1) / threadsPerBlock;
        vectorAddKernel<<<blocksPerGrid, threadsPerBlock>>>(d_a, d_b, d_c, n);

        cudaMemcpy(c, d_c, size, cudaMemcpyDeviceToHost);

        cudaFree(d_a);
        cudaFree(d_b);
        cudaFree(d_c);
    }

    // Scale vector
    static void scale(float *data, float factor, int n) {
        float *d_data;
        size_t size = n * sizeof(float);

        cudaMalloc(&d_data, size);
        cudaMemcpy(d_data, data, size, cudaMemcpyHostToDevice);

        int threadsPerBlock = BLOCK_SIZE;
        int blocksPerGrid = (n + threadsPerBlock - 1) / threadsPerBlock;
        simpleKernel<<<blocksPerGrid, threadsPerBlock>>>(d_data, n);

        cudaMemcpy(data, d_data, size, cudaMemcpyDeviceToHost);
        cudaFree(d_data);
    }
};

// ============================================================================
// Main Function
// ============================================================================

int main(int argc, char **argv) {
    printf("CUDA Comprehensive Test\n");
    printf("=======================\n\n");

    // Get device information
    GpuDevice device(0);
    device.printInfo();
    printf("\n");

    // Test vector addition
    const int n = N;
    size_t size = n * sizeof(float);

    float *h_a = (float *)malloc(size);
    float *h_b = (float *)malloc(size);
    float *h_c = (float *)malloc(size);

    // Initialize arrays
    for (int i = 0; i < n; i++) {
        h_a[i] = (float)i;
        h_b[i] = (float)(n - i);
    }

    // Allocate device memory
    float *d_a, *d_b, *d_c;
    cudaMalloc(&d_a, size);
    cudaMalloc(&d_b, size);
    cudaMalloc(&d_c, size);

    // Copy to device
    cudaMemcpy(d_a, h_a, size, cudaMemcpyHostToDevice);
    cudaMemcpy(d_b, h_b, size, cudaMemcpyHostToDevice);

    // Launch kernel
    int threadsPerBlock = BLOCK_SIZE;
    int blocksPerGrid = (n + threadsPerBlock - 1) / threadsPerBlock;

    printf("Launching vectorAddKernel with %d blocks, %d threads per block\n",
           blocksPerGrid, threadsPerBlock);

    vectorAddKernel<<<blocksPerGrid, threadsPerBlock>>>(d_a, d_b, d_c, n);

    // Copy result back
    cudaMemcpy(h_c, d_c, size, cudaMemcpyDeviceToHost);

    // Verify results
    bool success = true;
    for (int i = 0; i < n; i++) {
        float expected = h_a[i] + h_b[i];
        if (fabs(h_c[i] - expected) > 1e-5) {
            success = false;
            printf("Error at index %d: %f != %f\n", i, h_c[i], expected);
            break;
        }
    }

    printf("Vector addition test: %s\n", success ? "PASSED" : "FAILED");

    // Test matrix multiplication
    const int width = 32;
    size_t matrixSize = width * width * sizeof(float);

    float *h_A = (float *)malloc(matrixSize);
    float *h_B = (float *)malloc(matrixSize);
    float *h_C = (float *)malloc(matrixSize);

    for (int i = 0; i < width * width; i++) {
        h_A[i] = 1.0f;
        h_B[i] = 1.0f;
    }

    float *d_A, *d_B, *d_C;
    cudaMalloc(&d_A, matrixSize);
    cudaMalloc(&d_B, matrixSize);
    cudaMalloc(&d_C, matrixSize);

    cudaMemcpy(d_A, h_A, matrixSize, cudaMemcpyHostToDevice);
    cudaMemcpy(d_B, h_B, matrixSize, cudaMemcpyHostToDevice);

    dim3 blockDim(16, 16);
    dim3 gridDim((width + blockDim.x - 1) / blockDim.x,
                 (width + blockDim.y - 1) / blockDim.y);

    matrixMultiplyKernel<<<gridDim, blockDim>>>(d_A, d_B, d_C, width);

    cudaMemcpy(h_C, d_C, matrixSize, cudaMemcpyDeviceToHost);

    // Verify matrix multiplication (all 1s * all 1s = all 32s)
    success = true;
    for (int i = 0; i < width * width; i++) {
        if (fabs(h_C[i] - width) > 1e-5) {
            success = false;
            break;
        }
    }

    printf("Matrix multiplication test: %s\n", success ? "PASSED" : "FAILED");

    // Cleanup
    free(h_a);
    free(h_b);
    free(h_c);
    free(h_A);
    free(h_B);
    free(h_C);

    cudaFree(d_a);
    cudaFree(d_b);
    cudaFree(d_c);
    cudaFree(d_A);
    cudaFree(d_B);
    cudaFree(d_C);

    printf("\nAll CUDA tests completed successfully!\n");

    return 0;
}
