# Cross-Language Interoperability Testing

`cddlc` can generate a set of canonical CBOR test vectors together with
per-language test harnesses that verify every supported serialisation backend
encodes and decodes those vectors identically.  This makes it straightforward
to confirm that a Rust encoder, a Python decoder, and a C decoder all agree on
the wire format for a given schema.

## Generating interop artefacts

Add `--interop` to any `cddlc` invocation:

```bash
cddlc my_schema.cddl --interop -o generated
```

You can combine it with a specific language target so you get both the
serialisation library and the interop harness in one pass:

```bash
cddlc my_schema.cddl -l c --interop -o generated
```

To limit which harnesses are emitted (e.g. when only some runtimes are
available in your CI environment):

```bash
cddlc my_schema.cddl --interop --interop-langs rust,python -o generated
```

### Output layout

```
generated/
  <schema>-cbor/          ← serialisation library (language-specific)
  <schema>-interop/
    vectors.json          ← canonical test vectors (human-readable)
    vectors.cbor          ← same vectors in binary CBOR
    rust/interop_test.rs
    c/test_interop.c
    cpp/test_interop.cpp
    csharp/InteropTests.cs
    nodejs/interop.test.ts
    python/test_interop.py
```

## What the vectors contain

`vectors.json` has one entry per type per interesting value combination.
Each entry records:

| Field | Description |
|---|---|
| `type` | CDDL type name |
| `description` | human label (e.g. `"default values"`, `"variant Temperature"`) |
| `cbor_hex` | hex-encoded canonical CBOR bytes |
| `fields` | expected decoded field values for assertion |

Example entry for `iot_sensor.cddl`:

```json
{
  "type": "SensorType",
  "description": "variant Temperature",
  "cbor_hex": "6b74656d7065726174757265",
  "fields": { "kind": "Temperature" }
}
```

## Running each harness

### Rust

Copy the generated harness into the library's `tests/` directory and run
`cargo test`:

```bash
cp generated/iot_sensor-interop/rust/interop_test.rs \
   generated/iot_sensor-cbor/tests/

cargo test --manifest-path generated/iot_sensor-cbor/Cargo.toml
```

### Python

Install the generated library, then run pytest against the harness:

```bash
pip install -e generated/iot_sensor-cbor
pytest generated/iot_sensor-interop/python/test_interop.py
```

### Node.js / TypeScript

Build the library first, then run Jest:

```bash
cd generated/iot_sensor-cbor
npm install && npm run build

cp ../iot_sensor-interop/nodejs/interop.test.ts .
npx jest interop.test.ts
```

### C# / .NET

Copy the harness into the library project and run `dotnet test`:

```bash
cp generated/iot_sensor-interop/csharp/InteropTests.cs \
   generated/iot_sensor-cbor/tests/

dotnet test generated/iot_sensor-cbor/
```

### C

The C harness is a self-contained `.c` file that `#include`s the generated
header.  Compile it against the library built by CMake:

```bash
# 1. Build the library
cmake -B generated/iot_sensor-cbor/build -S generated/iot_sensor-cbor
cmake --build generated/iot_sensor-cbor/build

# 2. Compile and run the interop test
cc -I generated/iot_sensor-cbor/include \
   -I generated/iot_sensor-cbor/build/_deps/nanocbor-src/include \
   generated/iot_sensor-interop/c/test_interop.c \
   generated/iot_sensor-cbor/build/libot_sensor_cbor.a \
   -o test_interop_c
./test_interop_c
```

### C++

Same pattern as C, linking against the C++ library:

```bash
cmake -B generated/iot_sensor-cbor-cpp/build -S generated/iot_sensor-cbor-cpp
cmake --build generated/iot_sensor-cbor-cpp/build

c++ -std=c++17 \
    -I generated/iot_sensor-cbor-cpp/include \
    -I generated/iot_sensor-cbor-cpp/build/_deps/nanocbor-src/include \
    generated/iot_sensor-interop/cpp/test_interop.cpp \
    generated/iot_sensor-cbor-cpp/build/libot_sensor_cbor_cpp.a \
    -o test_interop_cpp
./test_interop_cpp
```

## What each harness verifies

Every test in every harness follows the same three-step pattern:

1. **Decode** the `cbor_hex` bytes using the generated decoder.
2. **Assert fields** match the values listed in `vectors.json`.
3. **Re-encode** the decoded value and assert the output bytes are
   byte-for-byte identical to the original hex.

This catches:

- Decoders that accept invalid CBOR or misread field types
- Encoders that produce non-canonical output (wrong map length, wrong key
  order, wrong integer encoding width)
- Schema drift — if the CDDL changes and the vectors are regenerated, any
  backend whose encoding has changed will fail the re-encode assertion

## Regenerating vectors after a schema change

Re-run the same `cddlc` command.  The vectors are derived deterministically
from the schema, so regenerating replaces all files in `<schema>-interop/`
with values consistent with the new schema.  All harnesses will then test
against the updated expected bytes.
