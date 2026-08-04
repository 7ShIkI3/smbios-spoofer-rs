# Apprendre Rust avec le SMBIOS Spoofer

Cours autodidacte. Chaque chapitre = un concept Rust + son application dans le projet.
Fais les chapitres dans l'ordre. Ne passe pas au suivant tant que tu n'as pas fait l'exercice.

---

## Chapitre 0 : Pourquoi Rust remplace C

Avant de taper une ligne, comprends POURQUOI.

En C, quand tu écris :

```c
char* ptr = (char*)0x1234;
*ptr = 'A';  // Boom si l'adresse est invalide. Pas d'erreur. Pas de warning.
```

Le compilateur C te dit "ok chef" et te laisse faire n'importe quoi. C'est pour ça que le code du spoofer original a ce commentaire dans `finder.c` :

```c
// FindBySignature is DISABLED — can cause page faults
```

En Rust, le même code :

```rust
// ERREUR DE COMPILATION : cannot dereference raw pointer outside unsafe
let ptr = 0x1234 as *mut u8;
*ptr = b'A';
```

Rust REFUSE de compiler ça sans que tu mettes `unsafe { *ptr = b'A'; }`. Et `unsafe` c'est un contrat : "je sais ce que je fais, je prends la responsabilité". Le compilateur te fait confiance pour 5 opérations précises (déréférencement de pointeur brut, appel FFI, mutation de statique mutable, union field access, inline assembly). Tout le reste est garanti safe.

**Les trois piliers que tu vas intérioriser :**

1. **Ownership** — chaque valeur a UN propriétaire. Pas de double-free, pas de use-after-free.
2. **Borrowing** — tu peux prêter (&T, immutable) ou prêter mutable (&mut T, exclusif). Jamais les deux en même temps.
3. **Lifetimes** — le compilateur prouve qu'aucune référence ne survit à ce qu'elle référence.

---

## Chapitre 1 : Ownership — le concept qui change tout

**Fichier de référence** : `src/main.rs` (la struct `SpooferContext`)

### La règle

En C tu gères la mémoire à la main. En Rust, le compilateur insère les `free()` pour toi, aux bons endroits, prouvés par le type system.

```rust
// C : tu alloues, tu oublies de libérer → fuite mémoire
// C : tu libères deux fois → crash
// C : tu utilises après libération → comportement indéfini

// Rust : le compilateur GARANTIT l'absence de ces trois bugs
```

### Exemple concret depuis notre projet

Compare le C et le Rust pour le contexte du spoofer :

```c
// persistence.c — variables globales éparpillées
static UINT8  g_SpoofedUUID[16] = {0};
static CHAR16 g_SystemSerial[64] = {0};
static CHAR16 g_BiosSerial[64] = {0};
static CHAR16 g_BaseboardSerial[64] = {0};
// ... 5 fichiers, 15 variables globales, qui modifie quoi ?
```

```rust
// main.rs — tout l'état dans UNE struct
struct SpooferContext {
    smbios_entry: Option<&'static SmbiosEntry>,  // Option = peut être absent
    config: SpoofConfig,                          // possédé (owned)
    orig_uuid: [u8; 16],                          // tableau sur la pile, pas de malloc
    loaded_from_storage: bool,
}

// Le contexte est créé une fois, passé par &mut aux fonctions qui en ont besoin.
// Pas de variable globale. Pas de surprise.
let mut ctx = SpooferContext::new();
patch_all(&mut ctx);   // &mut = accès exclusif, le compilo vérifie
print_summary(&ctx);   // & = accès partagé, le compilo vérifie
```

### Exercice 1.1

Dans le fichier `src/main.rs`, trouve toutes les utilisations de `ctx` et identifie :
- Où est-ce que `ctx` est **possédé** (owned) ?
- Où est-ce qu'il est **emprunté immuablement** (`&ctx`) ?
- Où est-ce qu'il est **emprunté mutablement** (`&mut ctx`) ?

**Réponse attendue** : tu dois trouver que `ctx` est créé dans `main()` (owned), passé en `&mut` à `wait_key_with_reset`, et en `&` à `print_summary`. Le compilateur vérifie que `&mut` et `&` n'existent jamais simultanément.

---

## Chapitre 2 : Types, Structs, et `#[repr(C)]`

**Fichier de référence** : `src/smbios.rs`

### Pourquoi `#[repr(C)]` est crucial

En Rust, le compilateur est libre de réorganiser les champs d'une struct pour optimiser l'alignement. Mais quand tu lis de la mémoire brute (les tables SMBIOS du firmware), l'ordre des octets est imposé par la spec.

```rust
// SANS #[repr(C)] : le compilo peut mettre 'serial_number' avant 'uuid'
// → ton mapping mémoire serait faux → valeurs incorrectes lues
struct SmbiosType1 {
    manufacturer: u8,
    uuid: [u8; 16],
    serial_number: u8,
}

// AVEC #[repr(C)] : l'ordre est GARANTI identique à la spec SMBIOS
#[repr(C)]
struct SmbiosType1 {
    pub header: SmbiosHeader,      // offset 0
    pub manufacturer: u8,          // offset 4
    pub product_name: u8,          // offset 5
    pub version: u8,               // offset 6
    pub serial_number: u8,          // offset 7
    pub uuid: [u8; 16],            // offset 8
    // ...
}
```

### `#[repr(C, packed)]` dans persistence.rs

Quand tu sérialises une structure pour l'écrire en NVRAM, tu veux EXACTEMENT la même taille que la version C, sans padding entre les champs.

```rust
#[repr(C, packed)]  // Pas de padding. sizeof(SpoofConfig) == somme des sizeof(champs)
pub struct SpoofConfig {
    pub magic: u32,                  // 4 bytes
    pub version: u32,                // 4 bytes
    pub uuid: [u8; 16],             // 16 bytes
    pub system_serial: [u16; 64],   // 128 bytes
    // ...
}
// Sans 'packed', le compilo ajouterait du padding entre uuid et system_serial
// pour aligner system_serial sur 2 bytes. La taille changerait. La version C
// lirait des données décalées → corruption.
```

### Exercice 2.1

Ouvre `src/smbios.rs`. Repère les structs `SmbiosEntry`, `SmbiosHeader`, `SmbiosType1`, `SmbiosType2`, `SmbiosType4`.

Question : Pourquoi `SmbiosType1.uuid` est à l'offset 8 ?

Aide : `SmbiosHeader` fait 4 bytes (u8 + u8 + u16). `manufacturer`, `product_name`, `version`, `serial_number` font chacun 1 byte. 4 + 1 + 1 + 1 + 1 = 8.

---

## Chapitre 3 : `Option<T>` — dire adieu à NULL

**Fichier de référence** : `src/finder.rs`, `src/smbios.rs`

### Le problème de NULL en C

```c
// C : n'importe quel pointeur peut être NULL à n'importe quel moment
SMBIOS_STRUCTURE_TABLE* entry = FindEntry();
// Si t'oublies de vérifier → crash ou corruption silencieuse
PatchAll(entry);  // Est-ce que entry peut être NULL ? MYSTÈRE.
```

En Rust, `NULL` n'existe pas. Si une valeur peut être absente, le type le DIT :

```rust
// Rust : le type encode la possibilité d'absence
fn find_smbios_entry(st: &SystemTable<Boot>) -> Option<&SmbiosEntry> {
    // retourne Some(entry) si trouvé, None sinon
}

// Tu ES OBLIGÉ de gérer les deux cas
match find_smbios_entry(&st) {
    Some(entry) => { /* trouvé, entry est garanti valide */ }
    None => { /* pas trouvé, entry n'existe pas */ }
}
```

### Pattern matching exhaustif

```rust
// Le compilo T'EMPÊCHE d'oublier un cas
let result = find_smbios_entry(&st);

// Ça NE COMPILE PAS :
// result.unwrap();  // panique si None, mais le compilo ne vérifie pas

// Ça compile, mais c'est risqué :
if let Some(entry) = result {  // le cas None est silencieusement ignoré
    // ...
}

// IDÉAL : match exhaustif
match result {
    Some(entry) => uefi::print!("Found v{}.{}\n", entry.major_version, entry.minor_version),
    None => {
        uefi::print!("FATAL: not found\n");
        return Status::NOT_FOUND;
    }
}
```

### Exercice 3.1

Dans `src/smbios.rs`, trouve la fonction `find_by_type`. Elle retourne `Option<SmbiosStruct>`. Dans `src/patch.rs`, les fonctions `patch_type1/2/4` l'appellent.

Question : Comment ces fonctions gèrent-elles le cas `None` ? Quel pattern Rust est utilisé ?

---

## Chapitre 4 : `Result<T, E>` — gérer les erreurs sans codes de retour

**Fichier de référence** : `src/persistence.rs`

### Le problème en C

```c
// C : code de retour magique, facile à ignorer
EFI_STATUS status = SaveToNvram(...);
// T'as oublié de vérifier status → les données sont perdues, silence radio
DoSomethingElse();  // continue comme si de rien n'était
```

### La solution Rust

```rust
// Rust : le type force la gestion d'erreur
fn save_to_nvram(rt: &RuntimeServices, config: &SpoofConfig) -> Result<(), Status> {
    // Ok(()) = succès
    // Err(Status::DEVICE_ERROR) = échec
}

// Pattern 1 : match explicite
match save_to_nvram(rt, &config) {
    Ok(()) => uefi::print!("Saved!\n"),
    Err(e) => uefi::print!("Error: {:?}\n", e),
}

// Pattern 2 : l'opérateur ? (propage l'erreur)
fn do_stuff(rt: &RuntimeServices) -> Result<(), Status> {
    save_to_nvram(rt, &config)?;  // si erreur, retourne immédiatement Err(...)
    save_to_disk(bs, &config)?;   // n'est exécuté que si le premier a réussi
    Ok(())
}

// Pattern 3 : ignorer explicitement (le _ dit "j'ai conscience de l'erreur")
let _ = save_to_nvram(rt, &config);  // best-effort, on continue quoi qu'il arrive
```

### Exercice 4.1

Dans `src/persistence.rs`, trouve la fonction `load_config`. Elle chaîne `load_from_nvram` puis `load_from_disk` avec `.or_else()`. Explique ce que fait cette chaîne. Pourquoi est-ce plus lisible que la version C équivalente dans `main.c` (lignes 447-463) ?

---

## Chapitre 5 : `unsafe` — le contrat avec le compilateur

**Fichier de référence** : `src/smbios.rs`, `src/patch.rs`

### La philosophie

`unsafe` ne désactive PAS le borrow checker. Il débloque 5 super-pouvoirs :

1. Déréférencer un pointeur brut (`*const T`, `*mut T`)
2. Appeler une fonction `unsafe` (y compris FFI)
3. Accéder/modifier une variable `static mut`
4. Accéder à un champ d'une `union`
5. Inline assembly (`asm!`)

Tout le reste (ownership, borrowing, lifetimes, pattern matching) fonctionne normalement dans un bloc `unsafe`.

### La règle d'or

Quand tu écris `unsafe { ... }`, tu dois pouvoir **prouver** (dans un commentaire) pourquoi c'est correct. Exemple :

```rust
// ✅ BON : le commentaire explique POURQUOI c'est safe
pub unsafe fn from_raw(ptr: *const u8) -> Option<Self> {
    // SAFETY: caller garantit que ptr est null OU pointe vers une structure
    // SMBIOS valide dans la mémoire firmware. Le firmware ne désalloue pas
    // ces tables tant que la machine est allumée.
    if ptr.is_null() {
        return None;
    }
    let type_ = unsafe { (*ptr.cast::<SmbiosHeader>()).type_ };
    Some(Self { ptr: SmbiosStructPtr { raw: ptr }, type_ })
}

// ❌ MAUVAIS : pas de justification
pub unsafe fn from_raw(ptr: *const u8) -> Option<Self> {
    let type_ = unsafe { (*ptr.cast::<SmbiosHeader>()).type_ };  // Pourquoi c'est OK ?
    Some(Self { ptr: SmbiosStructPtr { raw: ptr }, type_ })
}
```

### Exercice 5.1

Dans `src/patch.rs`, la fonction `patch_all` appelle `patch_type1`, `patch_type2`, `patch_type4`. Chacune contient des blocs `unsafe`. Pour CHAQUE bloc `unsafe` dans `patch_type1`, écris en une phrase pourquoi il est justifié.

---

## Chapitre 6 : Unions — l'équivalent Rust du `union` C

**Fichier de référence** : `src/smbios.rs` (`SmbiosStructPtr`)

### Pourquoi une union

Les tables SMBIOS sont une liste de structures de types différents. À une adresse mémoire donnée, on peut trouver un Type 0, Type 1, Type 2, ou Type 4. Une union permet de voir la même mémoire sous plusieurs angles.

```rust
#[repr(C)]
pub union SmbiosStructPtr {
    pub raw: *const u8,              // accès octet par octet
    pub header: *const SmbiosHeader, // vu comme en-tête générique (les 4 premiers bytes)
    pub type1: *const SmbiosType1,   // vu comme Type 1
    pub type2: *const SmbiosType2,   // vu comme Type 2
    pub type4: *const SmbiosType4,   // vu comme Type 4
}
```

En Rust, accéder à un champ d'une union est **toujours unsafe** — le compilateur ne sait pas quel champ est actif. C'est à toi de le savoir (via le champ `type_` du header).

```rust
// Le wrapper safe qui vérifie le type avant d'accéder à l'union
pub fn as_type1(&self) -> Option<&SmbiosType1> {
    if self.type_ == SMBIOS_TYPE_SYSTEM {
        unsafe { Some(&*self.ptr.type1) }  // On SAIT que c'est un Type1
    } else {
        None  // Pas un Type1, on ne touche pas à l'union
    }
}
```

### Exercice 6.1

Implémente `as_type0()` et `as_type3()` pour `SmbiosStruct`. Le Type 0 utilise `SmbiosType0` (déjà défini dans `smbios.rs`). Pour le Type 3, tu dois d'abord définir la struct `SmbiosType3` avec les champs : `header`, `manufacturer` (u8), `type_` (u8), `version` (u8), `serial_number` (u8), `asset_tag` (u8).

---

## Chapitre 7 : Modules — organiser son code

**Fichier de référence** : tous les `src/*.rs` + `main.rs`

### La structure

En Rust, un fichier = un module. Le `mod` dans `main.rs` déclare les modules :

```rust
// main.rs
mod utils;        // → src/utils.rs (ou src/utils/mod.rs)
mod smbios;       // → src/smbios.rs
mod finder;       // → src/finder.rs
mod patch;        // → src/patch.rs
mod persistence;  // → src/persistence.rs
```

Tout est privé par défaut. `pub` rend accessible aux autres modules.

```rust
// smbios.rs
pub fn find_by_type(...)  // ← accessible depuis patch.rs, main.rs
fn table_length(...)       // ← privé, utilisable seulement dans smbios.rs
```

### `use` — importer ce dont on a besoin

```rust
// main.rs
use crate::smbios;          // utiliser smbios::SmbiosEntry, smbios::find_by_type, etc.
use crate::persistence::SpoofConfig;  // utiliser SpoofConfig directement
use uefi::prelude::*;       // wildcard : tout ce que uefi::prelude exporte
```

### Exercice 7.1

Dans `src/patch.rs`, on utilise `crate::smbios::edit_string`. Pourquoi on écrit `crate::smbios::` et pas juste `smbios::` ? (Indice : `crate` = la racine de ton projet, comme `src/`).

---

## Chapitre 8 : `const` et `static` — les variables globales en Rust

**Fichier de référence** : `src/utils.rs`

### `const` — valeur constante, inline à la compilation

```rust
const SERIAL_CHARSET: &[u8] = b"123456789ABCDEFGHJKMNPQRSTUVWXYZ";
// Compilateur remplace chaque utilisation par la valeur directement.
// Pas d'adresse mémoire. Pas de coût runtime.
```

### `static` — valeur à une adresse mémoire fixe

```rust
static mut RANDOM_SEED: u32 = 0;
// Une SEULE instance en mémoire. Adresse fixe.
// 'mut' = peut être modifiée → nécessite unsafe pour lire/écrire.
```

### Pourquoi `static mut` est unsafe

```rust
// Deux threads pourraient écrire en même temps → data race
// En UEFI on est single-thread, donc c'est safe, mais le compilo
// ne peut pas le savoir → unsafe obligatoire.
pub fn random_u32() -> u32 {
    unsafe {
        RANDOM_SEED = RANDOM_SEED.wrapping_mul(1103515245).wrapping_add(12345);
        RANDOM_SEED
    }
}
```

### Exercice 8.1

Dans `src/utils.rs`, la fonction `random_init` utilise `GetTime` et `GetNextMonotonicCount` pour initialiser la graine. Pourquoi on fait ça ? Que se passe-t-il si on initialise toujours avec la même graine ?

---

## Chapitre 9 : Lifetimes — "cette référence vit combien de temps ?"

**Fichier de référence** : `src/finder.rs`

### Le concept

Le compilateur Rust prouve qu'aucune référence ne survit à la donnée qu'elle référence.

```rust
// Erreur de compilation classique :
fn give_me_a_ref() -> &SmbiosEntry {
    let entry = SmbiosEntry { /* ... */ };  // entry vit dans cette fonction
    &entry  // ERREUR : on retourne une référence vers une variable locale
}           // entry est détruite ici → la référence pointe vers du vide
```

### Dans notre code

```rust
pub fn find_smbios_entry(st: &SystemTable<Boot>) -> Option<&'static SmbiosEntry> {
    //                                           ^^^^^^^
    // 'static = "cette référence vit pour toute la durée du programme"
    // C'est correct car les tables SMBIOS sont dans la mémoire firmware,
    // qui n'est jamais désallouée tant que la machine tourne.
}
```

### La lifetime implicite

```rust
fn find_by_type(entry: &SmbiosEntry, type_: u8, index: usize) -> Option<SmbiosStruct>
// Pas de lifetime explicite → le compilo infère que la référence dans
// SmbiosStruct ne survit pas à entry.
```

### Exercice 9.1

Dans `src/persistence.rs`, `save_to_nvram` prend `rt: &RuntimeServices` et `config: &SpoofConfig`. Le compilateur vérifie que ces références restent valides pendant tout l'appel. Pourquoi `config` ne pourrait-il PAS être une référence mutable ici ?

---

## Chapitre 10 : `no_std` — programmer sans stdlib

**Fichier de référence** : `src/main.rs` (les deux premières lignes)

### Pourquoi

En UEFI, il n'y a pas de système d'exploitation. Pas de fichiers, pas de heap par défaut, pas de threads, pas de `println!` standard. La crate `uefi` fournit ses propres versions.

```rust
#![no_std]   // Pas de stdlib Rust
#![no_main]  // Pas de fn main() classique — on utilise l'entry point UEFI

// Au lieu de :
// fn main() { println!("Hello"); }

// On a :
#[entry]
fn main(_image_handle: Handle, mut st: SystemTable<Boot>) -> Status {
    uefi::print!("Hello from UEFI!\n");
    Status::SUCCESS
}
```

### Ce que tu perds avec `no_std`

- `Vec<T>`, `String`, `HashMap` — pas de heap sans allocateur
- `std::fs`, `std::net`, `std::thread` — pas d'OS
- `println!` standard — remplacé par `uefi::println!`

### Ce que tu gardes

- `Option<T>`, `Result<T,E>`, `match`, `if let`
- Slices (`&[T]`), arrays (`[T; N]`)
- Itérateurs, closures
- `core::mem`, `core::ptr`, `core::slice`

### Exercice 10.1

Dans `src/utils.rs`, on utilise `&[u8]` pour la table HEX. C'est un slice, pas un Vec. Pourquoi un Vec ne fonctionnerait-il pas ici (sans allocateur) ?

---

## Chapitre 11 : LCG et génération aléatoire

**Fichier de référence** : `src/utils.rs`

### Le LCG (Linear Congruential Generator)

C'est le générateur le plus simple qui existe :

```
seed_next = (seed * A + C) mod M
```

Notre version (identique à la version C) :

```rust
pub fn random_u32() -> u32 {
    unsafe {
        RANDOM_SEED = RANDOM_SEED.wrapping_mul(1103515245).wrapping_add(12345);
        RANDOM_SEED
    }
}
```

`wrapping_mul` et `wrapping_add` évitent le panic en cas de overflow (comportement voulu pour un LCG).

### Pourquoi pas `rand` ?

En `no_std` + UEFI, on n'a pas accès aux crates standard. Même si on pouvait, `rand` nécessite une source d'entropie que l'UEFI ne fournit pas nativement (pas de `/dev/urandom`). On utilise `GetTime` + `GetNextMonotonicCount` pour l'initialisation.

### Exercice 11.1

La génération UUID v4 utilise des bits spécifiques :

```rust
uuid[6] = (uuid[6] & 0x0F) | 0x40; // Version 4
uuid[8] = (uuid[8] & 0x3F) | 0x80; // Variant RFC 4122
```

Question : si on oublie ces deux lignes, l'UUID généré sera-t-il techniquement invalide ? Pourquoi le spoofer s'en fiche ?

---

## Chapitre 12 : Travailler avec des slices au lieu de pointeurs

**Fichier de référence** : `src/persistence.rs` (save/load)

### C vs Rust : sérialisation

```c
// C : on cast un pointeur et on prie
NVRAM_SPOOF_CONFIG config;
gRT->SetVariable(L"SmbiosSpoof", &guid, attrs, sizeof(config), &config);
// Si sizeof(config) est faux → corruption silencieuse
```

```rust
// Rust : le slice garantit la taille via le type system
let data = unsafe {
    core::slice::from_raw_parts(
        config as *const SpoofConfig as *const u8,
        mem::size_of::<SpoofConfig>(),  // taille calculée par le compilo
    )
};
rt.set_variable(var_name, &guid, attrs, data)?;
// Si SpoofConfig change, mem::size_of s'adapte automatiquement
```

### Exercice 12.1

Dans la même fonction, on utilise `mem::size_of::<SpoofConfig>()`. Pourquoi est-ce mieux que `sizeof(NVRAM_SPOOF_CONFIG)` en C ? (Réfléchis à ce qui se passe si on ajoute un champ à la struct.)

---

## Chapitre 13 : Pattern matching avancé

**Fichier de référence** : `src/main.rs` (wait_key_with_reset)

### Match sur des structures complexes

```rust
match key {
    // Touche imprimable
    uefi::proto::console::text::Key::Printable(c) => {
        if c == 'r' || c == 'R' {
            // Reset
        } else {
            break;
        }
    }
    // Tout le reste (touches spéciales)
    _ => break,
}
```

### If let — pour les cas simples

```rust
// Au lieu de :
match find_smbios_entry(&st) {
    Some(entry) => { /* faire un truc */ }
    None => {}
}

// On écrit :
if let Some(entry) = find_smbios_entry(&st) {
    // faire un truc
}
```

### Exercice 13.1

Dans `src/main.rs`, la fonction `wait_key_with_reset` utilise un `match` dans une boucle `loop`. Que se passe-t-il quand on appuie sur 'R' ? Pourquoi on fait `continue` au lieu de `break` ?

---

## Projet final : Ajoute le Type 3 (Chassis)

Tu as maintenant tous les outils. Voici ton Kata final :

1. Définis `SmbiosType3` dans `smbios.rs` (regarde la spec SMBIOS pour les champs, ou inspire-toi de `SmbiosType2`)
2. Ajoute `type3: *const SmbiosType3` dans l'union `SmbiosStructPtr`
3. Ajoute `as_type3()` dans `impl SmbiosStruct`
4. Crée `patch_type3()` dans `patch.rs`
5. Ajoute l'appel dans `patch_all()`
6. Ajoute les champs `chassis_serial` et `chassis_manufacturer` dans `SpoofConfig`
7. Mets à jour la persistance
8. Mets à jour l'affichage du résumé dans `main.rs`

**Quand tu as fini, envoie-moi ton code. Je le review.**

---

## Aide-mémoire : mappings C → Rust

| C | Rust |
|---|---|
| `UINT8` | `u8` |
| `UINT16` | `u16` |
| `UINT32` | `u32` |
| `UINT64` | `u64` |
| `INTN` | `isize` |
| `CHAR8` | `u8` (ASCII) |
| `CHAR16` | `u16` (UTF-16) |
| `BOOLEAN` | `bool` |
| `VOID*` | `*const u8` ou `*mut u8` |
| `NULL` | `Option<T>` (None) |
| `CopyMem(dst, src, n)` | `dst[..n].copy_from_slice(&src[..n])` |
| `ZeroMem(ptr, n)` | `ptr.write_bytes(0, n)` |
| `CompareGuid(a, b)` | `a == b` |
| `StrCmp(a, b)` | comparaison manuelle slice |
| `sizeof(T)` | `core::mem::size_of::<T>()` |
| `return EFI_SUCCESS` | `Status::SUCCESS` (ou `Ok(())`) |
| `return EFI_NOT_FOUND` | `Status::NOT_FOUND` (ou `Err(...)`) |
| `if (EFI_ERROR(s))` | `if result.is_err()` |
| `gST->ConOut->ClearScreen(...)` | `st.stdout().clear()` |
| `gBS->Stall(us)` | `bs.stall(us)` |
| `#define CONSTANT valeur` | `const CONSTANT: type = valeur;` |
