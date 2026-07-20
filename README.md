# AgamaLang

**Промежуточный язык программирования (mid-level)** с C-подобным синтаксисом. Компилируется напрямую в x86-64 машинный код — создаёт .exe для Windows без ассемблера и линкера.

```
fn main() {
    println("Привет из AgamaLang!");
}
```

## Быстрый старт

```powershell
cargo build
cargo run -- examples\hello.aga
.\hello.exe
```

Компилятор создаёт .exe в **текущей папке** (не в `examples/`).

---

## Синтаксис языка

### Функции

Программа состоит из функций. Точка входа — `fn main()`.

```
fn main() {
    println("Hello!");
}

fn add(a: int, b: int) -> int {
    return a + b;
}
```

- `-> int` задаёт возвращаемый тип (по умолчанию `void`)
- `return` без значения — для `void` функций

### Переменные

```
let x: int = 42;
let name: char = 'A';
let flag: bool = true;
let ptr: *int = null;
```

- Все переменные хранятся на стеке, 4 байта каждая
- Тип указывается через `:` после имени
- Инициализация опциональна: `let x: int;`
- Переменные можно изменять: `x = 10;`

### Типы данных

| Тип | Описание | Размер |
|-----|----------|--------|
| `int` | Целое число (i32) | 4 байта |
| `char` | Символ (u8) | 4 байта |
| `bool` | Логический (0/1) | 4 байта |
| `void` | Пустой тип | — |
| `*T` | Указатель на T | 8 байт |
| `T[N]` | Статический массив N элементов T | N × 4 |
| `T[]` | Динамический массив T | 8 байт |
| `struct` | Пользовательский тип | сумма полей |

### Числа

```
let a: int = 42;
let b: int = -17;
let c: int = 0;
```

Только целые числа (`i64` в AST, `i32` в памяти на стеке).

### Символы и строки

```
let c: char = 'A';
let name: char = '\n';   // перевод строки
let tab: char = '\t';    // табуляция
let nul: char = '\0';    // нуль-символ

print("Hello");           // строка без перевода строки
println("Hello");         // строка с \r\n на конце
```

Поддерживаемые escape-последовательности: `\n` `\r` `\t` `\\` `\"` `\'` `\0`

### Арифметика

```
let sum: int = a + b;
let diff: int = a - b;
let prod: int = a * b;
let quot: int = a / b;
let rem: int = a % b;
```

### Сравнение (результат — 0 или 1)

```
if a == b { }
if a != b { }
if a <  b { }
if a >  b { }
if a <= b { }
if a >= b { }
```

### Логические операторы

```
if a && b { }   // И
if a || b { }   // ИЛИ
if !flag { }    // НЕ
```

### Указатели

```
let x: int = 42;
let p: *int = &x;   // взять адрес
let v: int = *p;    // разыменовать
let n: *int = null; // нулевой указатель
```

---

## Управляющие конструкции

### `if` / `else`

```
if (x > 0) {
    println("positive");
} else if (x == 0) {
    println("zero");
} else {
    println("negative");
}
```

⚠️ **Условие обязательно в скобках!**

### `while`

```
let mut i: int = 0;
while i < 10 {
    i = i + 1;
}
```

### `for`

```
for (let i: int = 0; i < 10; i = i + 1) {
    println(i);
}
```

### `break` / `continue`

```
while true {
    if done {
        break;
    }
    if skip {
        continue;
    }
    process();
}
```

### `return`

```
fn add(a: int, b: int) -> int {
    return a + b;
}
```

В `main` вызов `return` автоматически завершает процесс через `ExitProcess`.

---

## Структуры

Объявление:

```
struct Point {
    x: int;
    y: int;
}
```

Создание и доступ:

```
let p: Point = Point {
    x: 10;
    y: 20;
};
let px: int = p.x;   // чтение поля
let sum: int = p.x + p.y;
```

Память под структуру выделяется на стеке (каждое поле — 4 байта).

---

## Массивы

### Литералы

```
let arr: int[3] = [10, 20, 30];
let first: int = arr[0];
```

### `sizeof`

```
let sz: int = sizeof(Point);   // 8 (2 поля × 4)
let sz2: int = sizeof(int);    // 4
```

---

## Встроенные функции

| Функция | Описание |
|---------|----------|
| `print(x)` | Печать строки/числа/булевого значения |
| `println(x)` | Печать с переводом строки |

На данный момент `print` и `println` принимают только **константы** (строковые литералы, числа, булевые значения), не переменные.

---

## Сборка и запуск

```powershell
cargo build                    # собрать компилятор
cargo run -- examples\file.aga               # → file.exe
cargo run -- examples\file.aga out.exe       # кастомное имя
.\file.exe                     # запустить
echo $LASTEXITCODE             # код возврата
```

### Известные предупреждения

При сборке компилятора выводится ~8 предупреждений о неиспользуемых полях (`column`, `return_type`, `param_type`, `imports` и др.). Это остатки от незавершённых возможностей — **их можно игнорировать**.

---

## Примеры

### `examples/exit.aga` — минимальная программа

```
fn main() {
}
```

→ exit code 0

### `examples/hello.aga` — приветствие

```
fn main() {
    println("Hello from AgamaLang!");
    println("This is a mid-level language.");
    println("It compiles directly to machine code!");
}
```

### `examples/struct.aga` — структуры

```
struct Token {
    kind: int;
    line: int;
    column: int;
}

fn main() {
    let t: Token = Token {
        kind: 42;
        line: 10;
        column: 30;
    };
    if t.kind == 42 {
        println("kind OK");
    }
}
```

---

## Архитектура компилятора

```
чтение → лексер → парсер → кодогенератор → PE writer → .exe
         │         │        │              │
     token.rs  parser.rs  codegen.rs    pe.rs
     lexer.rs   ast.rs
```

- **Лексер**: разбивает исходник на токены
- **Парсер**: рекурсивный спуск, строит AST
- **Кодогенератор**: обходит AST, генерирует x86-64 machine code
- **PE writer**: упаковывает код в формат PE (.exe), вручную строит таблицы импорта (ILT/IAT)

---

## PE-структура .exe

| Секция | RVA | Содержимое |
|--------|-----|------------|
| `.text` | 0x1000 | Машинный код |
| `.data` | 0x2000 | Строковые константы |
| `.idata` | 0x3000 | Таблицы импорта Win32 API |

- `ImageBase = 0x400000`
- Используются: `kernel32.dll` → `GetStdHandle`, `WriteFile`, `ExitProcess`
- Вызов DLL через `call [rip+offset]` (`FF 15`)

---

## Планы

- ✅ Структуры и member access
- ✅ break/continue
- ✅ Индексация массивов
- ✅ sizeof
- 🔜 Runtime int→string для print с переменными
- 🔜 File I/O (ReadFile/WriteFile)
- 🔜 Динамическая память (HeapAlloc)
- 🔜 Самокомпиляция (self-hosting)

---

*AgamaLang — mid-level compiled language, 2026*
