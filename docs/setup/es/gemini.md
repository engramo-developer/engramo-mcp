# Conecta EngrAmo a Gemini

[← Todas las guías](README.md)

Funciona en gemini.google.com y, una vez conectado, también en la app móvil de Gemini. Tardas unos 3 minutos.

> Esta conexión no tiene audio en las tarjetas. Para el audio, consulta
> [Antigravity con audio](antigravity-audio.md).

## Qué necesitas

Por ahora Google solo permite aplicaciones personalizadas en Gemini a quienes:

- tienen **18 años o más** y están **en EE. UU.**
- usan una cuenta de Google **personal** (no de trabajo ni de centro educativo)
- tienen activado el **historial** de Gemini (también llamado **Keep Activity**, Guardar actividad).
  Compruébalo en <https://myactivity.google.com/product/gemini>. Si está desactivado, las aplicaciones
  personalizadas no funcionan.

Por ahora las aplicaciones personalizadas solo funcionan en inglés, así que escribe a Gemini en inglés.
También necesitas una cuenta de EngrAmo (créala en <https://study.engramo.app> si no la tienes).

## Pasos

1. En un ordenador, abre <https://gemini.google.com> e inicia sesión.
2. Haz clic en **Settings** (Ajustes, abajo a la izquierda) y luego en **Connected Apps** (Aplicaciones
   conectadas). (En algunas versiones está dentro de **Personal Intelligence**. Atajo:
   <https://gemini.google.com/apps>.)
3. En **Custom apps** (Aplicaciones personalizadas), haz clic en **Add a custom app** (Añadir una aplicación
   personalizada).
4. Escribe la dirección: `https://mcp.engramo.app`
5. Haz clic en **Next** (Siguiente).
6. Se abre la página de inicio de sesión de EngrAmo. Entra con tu **correo y contraseña de EngrAmo** y haz
   clic en **Approve** (Aprobar).
7. Vuelves a Gemini y EngrAmo aparece en **Custom apps**.

Si el paso 5 muestra un error, prueba la dirección con una barra al final: `https://mcp.engramo.app/`

## Comprueba que funciona

Abre un chat nuevo, escribe `@`, elige **EngrAmo** y luego escribe:

```text
Show my EngrAmo catalogs.
```

Escribir `@EngrAmo` asegura que Gemini use EngrAmo en ese mensaje. Gemini te pide confirmación antes de
cambiar algo en tu cuenta (por ejemplo, al crear tarjetas).

## Desconectar

En Gemini: **Settings → Connected Apps** y, en **Custom apps**:

- desactiva el interruptor junto a EngrAmo para pausarlo, o
- haz clic en **More details → Remove app** (Más detalles → Quitar aplicación) para eliminarlo por completo.

O en la app de EngrAmo: **Settings → Connected apps**.
