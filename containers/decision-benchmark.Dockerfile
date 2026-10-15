# Optional local decision-model evaluation only; not the shipping Rust runtime.
# Build with an empty allowlisted context, never the workspace or a model directory.
FROM python:3.12-slim@sha256:2b4f19dae3a777dfc3b76730bda1e82e1f66ab2a2686fa93ca78edbfb4f04ffe AS anyjev
ENV PIP_NO_CACHE_DIR=1 PYTHONDONTWRITEBYTECODE=1 CMAKE_BUILD_PARALLEL_LEVEL=6
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl cmake ninja-build g++ pkg-config libvulkan-dev \
    glslc spirv-headers mesa-vulkan-drivers \
    && rm -rf /var/lib/apt/lists/*
RUN pip install numpy==2.5.4 Jinja2==3.1.6 diskcache==5.6.3 \
    typing-extensions==4.16.0 jsonschema==4.25.1
RUN curl --fail --location --retry 2 --max-time 300 \
    https://files.pythonhosted.org/packages/ec/e9/e7de2b0463ea3ffbf0ede6cb21b58c1258a8f6521aae45ca773a59fe7cf3/llama_cpp_python-0.3.36.tar.gz \
    -o /tmp/llama-python.tar.gz \
    && python -c 'import hashlib; assert hashlib.file_digest(open("/tmp/llama-python.tar.gz","rb"),"sha256").hexdigest()=="832db0699007f1be95a7e41ef12e88926b02ba836461e36a36372db2760c1a2e"' \
    && mkdir /opt/llama-python \
    && tar -xzf /tmp/llama-python.tar.gz --strip-components=1 -C /opt/llama-python \
    && CMAKE_ARGS='-DGGML_VULKAN=ON -DGGML_NATIVE=OFF' pip install --no-deps /opt/llama-python \
    && rm /tmp/llama-python.tar.gz
RUN curl --fail --location --retry 2 --max-time 300 \
    https://codeload.github.com/nokia-applied-research/AnyJev/tar.gz/b03158e9ba18e84c01054aaa393481925088db07 \
    -o /tmp/anyjev.tar.gz \
    && mkdir /opt/anyjev \
    && tar -xzf /tmp/anyjev.tar.gz --strip-components=1 -C /opt/anyjev \
    && pip install --no-deps /opt/anyjev \
    && rm /tmp/anyjev.tar.gz \
    && pip freeze > /opt/decision-environment.txt

FROM anyjev AS kev
# Install the explicit CPU wheel first so the resolver does not pull CUDA runtimes.
RUN pip install torch==2.8.0 --index-url https://download.pytorch.org/whl/cpu
RUN pip install transformers==5.19.0 peft==0.21.2 accelerate==1.15.0 \
    datasets==5.1.0 scikit-learn==1.9.1 pydantic==2.14.1 \
    fastapi==0.143.1 typesafe-sdk==0.7.5 uvicorn==0.54.0
RUN curl --fail --location --retry 2 --max-time 300 \
    https://codeload.github.com/jaredpalmer/kev/tar.gz/dd375f27fd61486c52204205e10a0d2e10f3b34a \
    -o /tmp/kev.tar.gz \
    && mkdir /opt/kev \
    && tar -xzf /tmp/kev.tar.gz --strip-components=1 -C /opt/kev \
    && pip install --no-deps /opt/kev \
    && rm /tmp/kev.tar.gz \
    && pip freeze > /opt/decision-environment.txt \
    && pip check
