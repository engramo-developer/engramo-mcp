# Conecta EngrAmo a ChatGPT

[← Todas las guías](README.md)

Funciona en chatgpt.com. Tardas unos 3 minutos.

> Esta conexión no tiene audio en las tarjetas. Para el audio, consulta
> [Claude Desktop con audio](claude-desktop-audio.md) o [Antigravity con audio](antigravity-audio.md).

## Qué necesitas

- Una cuenta de ChatGPT. Para añadir tu propia aplicación hace falta el **modo desarrollador**, que no está
  en todos los planes. Si no lo ves en el paso 2, tu plan no lo incluye.
- Una cuenta de EngrAmo (créala en <https://study.engramo.app> si no la tienes)

> EngrAmo todavía no está en el directorio de aplicaciones de ChatGPT. Cuando lo esté, podrás buscar
> **EngrAmo** en **Apps** y hacer clic en **Connect**, sin modo desarrollador. Hasta entonces, sigue estos pasos.

## Pasos

1. En un ordenador, abre <https://chatgpt.com> e inicia sesión.
2. Haz clic en tu nombre (abajo a la izquierda) → **Settings** (Configuración) → **Apps & Connectors**
   (Aplicaciones y conectores; en algunas versiones, **Connectors**) → **Advanced settings** (Configuración
   avanzada) y activa **Developer mode** (Modo desarrollador).
3. De vuelta en **Apps & Connectors**, haz clic en **Create** (Crear).
4. Rellena:
   - **Name** (Nombre): `EngrAmo`
   - **MCP server URL** (URL del servidor MCP): `https://mcp.engramo.app/`
   - **Authentication** (Autenticación): `OAuth`
5. Marca la casilla que confirma que confías en esta aplicación y haz clic en **Create**.
6. Se abre la página de inicio de sesión de EngrAmo. Entra con tu **correo y contraseña de EngrAmo** y haz
   clic en **Approve** (Aprobar).
7. Vuelves a ChatGPT y EngrAmo aparece como conectado.

## Comprueba que funciona

1. Abre un chat nuevo.
2. Haz clic en **+** junto al cuadro de mensaje, abre **More** (Más) o **Developer mode** y elige **EngrAmo**.
3. Escribe:

   ```text
   Muéstrame mis catálogos de EngrAmo.
   ```

ChatGPT te pide confirmación antes de cambiar algo en tu cuenta (por ejemplo, al crear tarjetas). Haz clic
en **Confirm** (Confirmar).

## Desconectar

En ChatGPT: **Settings → Apps & Connectors → EngrAmo → Delete** (Eliminar). O en la app de EngrAmo:
**Settings → Connected apps**.
