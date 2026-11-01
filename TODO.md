# Features To Implement

## Replace/Update CUDA backend for reworked SPMT

Adapt CUDA code generation and orchestration to consume the reworked SPMT representation. With the renewed code structure similar to the new RCL backend. Validate with a validation frontend against RCL.

## Implement CU scheduling algorithms to optimize kernels and resource utilization

Develop scheduling strategies for compute units (CUs) that account for dependencies, kernel launch overhead, and (potentially) available compute and memory resources. Compare the strategies on representative workloads to confirm they improve execution efficiency without changing results.

## Explore SIMD possibilities with auto-vectorization

Investigate whether generated CPU code can expose independent density-function calculations to compiler auto-vectorization. Identify suitable loop and data layouts, implement promising changes, and benchmark them against the current generated code.

## Implement General Interpolation CU for optimizations

Introduce a reusable compute unit for interpolation that can represent common interpolation operations in the SPMT pipeline. Integrate it with the relevant transformations and scheduling, and confirm it produces equivalent results while enabling optimization opportunities.

## Add OpenCL backend for reworked SPMT

Adapt or implement OpenCL code generation and orchestration for the reworked SPMT representation. Integrate the backend with compiler configuration and outputs, then validate generated kernels on representative world-generation programs.

## Implement Parser for raw SPMT

Develop a parser that can read and interpret raw SPMT representations, converting them into the internal reworked SPMT format.

## Stabilize intrinsics

Ensure that the intrinsic functions used in the reworked SPMT pipeline are well-defined, consistent, and reliable across different backends. Implement necessary checks, documentation, and tests to guarantee their correct behavior and integration with the rest of the system.
