# The Standard Library and System Namespaces

Fable ships with batteries of two kinds. **System namespaces** — `math`,
`fs`, `os`, and the numeric namespaces of the next chapter — are implemented
in Rust, always present, and used without an import, like `math.sqrt`. The
**standard library** — `std.json`, `std.set`, and friends — is written in
Fable, compiled into the binary, and brought in with `import`. Nothing is on
disk to install either way.

## `fs` and `os`: touching the world

A program that only reads stdin and writes stdout can't do much glue work.
The `fs` and `os` namespaces cover the essentials, and everything fallible
returns `Result[_, String]`, so it composes with `?`:

```fable
fs.write("greeting.txt", "hello\nworld\n").unwrap();
let text = fs.read("greeting.txt").unwrap();
println(text.trim().split("\n").len());
println(fs.exists("greeting.txt"));

match os.run("echo", ["hi"]) {
    Ok(t) -> println("exit {t.0}: {t.1.trim()}"),
    Err(e) -> println("failed: {e}"),
}
```

```text
2
true
exit 0: hi
```

The full set: `fs.read`, `fs.write`, `fs.append`, `fs.exists`, `fs.is_dir`,
`fs.list_dir` (sorted), `fs.create_dir` (recursive), `fs.remove`, and
`fs.read_bytes`/`fs.write_bytes` for the `Bytes` of chapter 5; plus
`os.args()`, `os.env(name)`, `os.run(cmd, args)`, `os.exit(code)`, and
`os.time()`. A subprocess returns its whole story in one tuple: `Ok` means
the process *ran* — exit code `t.0`, stdout `t.1`, stderr `t.2` — while `Err`
means it could not be launched at all.

## The standard library

The `std.` prefix is reserved and needs no path. Each module is ordinary
Fable, readable under `std/` in the repository, and follows the same `pub`
rules as your code. The most useful additions to what the builtins already
give you:

**A string builder.** Chapter 5 showed that growing a string with `+=` in a
loop is quadratic. `strings.Builder` wraps the collect-then-join pattern in
an object you push onto:

```fable
import std.strings;

let b = strings.builder();
b.push("Hello");
b.push(", world");
println("{b.build()} ({b.len()} chars)");
```

```text
Hello, world (12 chars)
```

**Collections beyond `List` and `Map`.** `std.set` is a `Set[T]` whose
`insert` returns whether the value was new — the one-call membership test:

```fable
import std.set;

let seen = set.new();
println(seen.insert(3));   // true: newly added
println(seen.insert(3));   // false: already present
seen.insert(1);
println(seen.to_list());   // insertion order
```

```text
true
false
[3, 1]
```

`std.deque` is a double-ended queue with `push_front`/`push_back` and
`pop_front`/`pop_back` (each an `Option`), and `std.lists` adds the
aggregates `List` leaves out — `sum`, `min`/`max` (returning `Option`),
`min_by`/`max_by`, and `fill`:

```fable
import std.lists;

println(lists.sum([1, 2, 3, 4]));
println(lists.max([3, 1, 4, 1, 5]));
```

```text
10
Some(5)
```

**JSON**, parsed and generated, with `?`-friendly accessors:

```fable
import std.json;

match json.parse("\{\"port\": 8080}") {
    Ok(cfg) -> println(cfg.get("port").and_then(|p| p.as_num())),
    Err(e) -> println("bad config: {e}"),
}
```

```text
Some(8080.0)
```

(`\{` writes a literal brace, since a bare `{` in a Fable string opens an
interpolation hole.) `json.stringify` and `json.pretty` go the other way.

**Lazy iterators**, built from nothing but structs and closures — no
interpreter support required. An `Iter[T]` computes nothing until a consumer
pulls:

```fable
import std.iter;

let touched = [];
let pipeline = iter.of([1, 2, 3, 4, 5]).map(|x| { touched.push(x); x * 10 });
println(touched);                     // [] — map hasn't run yet
println(pipeline.take(2).collect());  // [10, 20]
println(touched);                     // [1, 2] — only what was pulled
```

```text
[]
[10, 20]
[1, 2]
```

Rounding out the set: `std.flags` is deliberately rigid CLI parsing
(`flag`, `value`, `value_or`, `positionals`), and `std.path` handles the
textual path chores (`join`, `base_name`, `dir_name`, `ext`, `strip_ext`).
The full method inventories live in the [spec](../docs/SPEC.md); the point
of the standard library is that all of it is Fable you can read, and none of
it cost the interpreter a line of Rust or the binary a dependency.

---

Previous: [Programs Across Files](07-modules.md) ·
Next: [Workers, `fft`, and the GPU](09-workers.md) ·
[Back to the index](README.md)
