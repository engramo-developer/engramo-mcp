# Conecta EngrAmo a Claude

[← Todas las guías](README.md)

Funciona en claude.ai, en Claude Desktop y en la app móvil de Claude. Lo configuras una vez y está
disponible en todos los sitios donde inicies sesión en Claude. Tardas unos 2 minutos.

> Esta conexión no tiene audio en las tarjetas. Para el audio, consulta
> [Claude Desktop con audio](claude-desktop-audio.md).

## Qué necesitas

- Una cuenta de Claude
- Una cuenta de EngrAmo (créala en <https://study.engramo.app> si no la tienes)

## Pasos

1. Abre la página de EngrAmo en el directorio de Claude: <https://claude.ai/directory/engramo>.
   (O en Claude: **Settings → Connectors → Browse** (Ajustes → Conectores → Explorar) y busca **EngrAmo**.)
2. Haz clic en **Connect** (Conectar).
3. Claude muestra un breve aviso de que EngrAmo es un conector de la comunidad. Continúa.
4. Se abre la página de inicio de sesión de EngrAmo. Entra con tu **correo y contraseña de EngrAmo**.
5. Haz clic en **Approve** (Aprobar) para que Claude pueda usar tu cuenta de EngrAmo.
6. Vuelves a Claude y EngrAmo aparece como conectado.

## Comprueba que funciona

Abre un chat nuevo y escribe:

```text
Muéstrame mis catálogos de EngrAmo.
```

La primera vez que Claude use EngrAmo te pedirá permiso. Haz clic en **Allow** (Permitir) o en
**Always allow** (Permitir siempre) para que no vuelva a preguntar.

## Si no encuentras EngrAmo en el directorio

Algunas cuentas de empresa ocultan el directorio. Añade EngrAmo por su dirección:

1. En Claude, abre **Settings → Connectors** (Ajustes → Conectores).
2. Haz clic en **Add custom connector** (Añadir conector personalizado).
3. Nombre: `EngrAmo`. URL: `https://mcp.engramo.app/`
4. Haz clic en **Add** (Añadir) y luego en **Connect**, y sigue desde el paso 4 de arriba.

En los planes Team o Enterprise puede que un administrador tenga que permitirlo antes.

## Desconectar

En Claude: **Settings → Connectors → EngrAmo → Disconnect** (Desconectar). O en la app de EngrAmo:
**Settings → Connected apps**.
