# 002 — Convenciones del workspace de Cargo

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 0

## Contexto

El prompt define la lista de crates y su dirección de dependencias, pero deja abiertos
detalles de organización del workspace.

## Decisiones

1. **Edición 2024, resolver 3, `rust-version = 1.88`.** Se usan *let chains*.
   `rust-toolchain.toml` fija el canal `stable` con `rustfmt` y `clippy`.
2. **Versiones centralizadas** en `[workspace.dependencies]`; cada crate usa
   `dep.workspace = true`. Lints también centralizados en `[workspace.lints]`
   (`unsafe_code = "deny"`: un `unsafe` justificado requiere `#[allow]` local + `// SAFETY:`).
3. **Dependencias solo cuando se usan.** Cargo avisa de dependencias no usadas y la regla
   del proyecto es cero warnings. Por eso los crates vacíos del hito 0 no declaran todavía
   las dependencias internas del diagrama; se añaden en el hito en que se usan, respetando
   siempre la dirección: `mv_core ← mv_materials ← mv_world ← mv_sim ← mv_physics`,
   `mv_render → mv_core, mv_world`, `mv_game → todos`.
4. **Benchmarks** en el paquete `benches/` (`mv_benches`), miembro del workspace, con los
   benches de Criterion en `benches/benches/*.rs`. Así se respeta la estructura del prompt
   y los benchmarks pueden cruzar crates.
5. **`mv_render` no depende de `winit`.** `GpuContext::new` acepta cualquier
   `wgpu::SurfaceTarget`, de modo que el renderer es reutilizable (otra capa de ventanas,
   render offscreen para tests, web a futuro).
6. **Binario `chemxel`** dentro del paquete `mv_game` (`cargo run -p mv_game` sigue
   funcionando; `mv_game` es además el `default-member`, así que basta `cargo run`).
7. **Perfil `dev` con `opt-level = 1`** para el código propio y `3` para las
   dependencias: la simulación es inutilizable sin optimizar. Perfil extra `profiling`
   (release + símbolos de línea) para Tracy.
8. **Configuración** en `assets/config.ron`, todos los campos opcionales con defaults
   (`#[serde(default, deny_unknown_fields)]`). Un campo desconocido es un error, para
   detectar erratas. La carpeta `assets/` se busca en `CHEMXEL_ASSETS_DIR`, luego en el
   directorio actual y luego junto al ejecutable y sus ancestros.

## Consecuencias

- `cargo build` / `clippy` deben quedar sin warnings, incluido el aviso de dependencias
  no usadas.
- Añadir un crate interno como dependencia implica también descomentarlo en
  `[workspace.dependencies]` del `Cargo.toml` raíz.
