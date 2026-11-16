# 001 — Logging y profiling con `tracing`

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 0

## Contexto

El prompt permite `log` + `env_logger` **o** `tracing-subscriber`, y pide `tracing` +
`tracy-client` detrás de un feature `profile`. Tener dos sistemas (uno para logs y otro
para spans de profiling) duplica la instrumentación.

## Decisión

- Toda la instrumentación usa **`tracing`** (macros `info!`, `debug!`, spans).
- La salida a consola la hace **`tracing-subscriber`** (`fmt` + `EnvFilter`, controlado
  por `RUST_LOG`). Los registros de crates que usan `log` (wgpu, winit) se reenvían
  automáticamente a `tracing` (feature `tracing-log`, activo por defecto).
- El feature `profile` de `mv_game` añade **`tracing-tracy`**, que conecta los spans de
  `tracing` con Tracy. Es una dependencia fuera de la lista del prompt: es el puente
  oficial entre `tracing` y Tracy y está construido sobre `tracy-client`, así que no
  añade un sistema nuevo, solo evita escribir ese puente a mano.

## Consecuencias

- Un único conjunto de macros para logs y profiling.
- `cargo run --release -p mv_game --features profile` transmite spans a Tracy.
  Para mejores símbolos: `cargo run --profile profiling -p mv_game --features profile`.
- Los spans en rutas calientes deben ser `trace_span!` (filtrables) para no costar nada
  en builds normales.
