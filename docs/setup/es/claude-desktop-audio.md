# Claude Desktop con audio en las tarjetas

[← Todas las guías](README.md)

Instala la extensión de EngrAmo en Claude Desktop para que Claude pueda crear tarjetas **y añadirles audio
hablado**. No hay que instalar otros programas ni editar archivos. Funciona en Mac, Windows y Linux,
también con el plan gratuito de Claude. Tardas unos 5 minutos.

## Qué necesitas

- La aplicación Claude Desktop: descárgala en <https://claude.ai/download>
- Tu **token de API de EngrAmo** y tu **clave de API de Gemini**: [consigue tus claves](keys.md)

## Pasos

1. Descarga el archivo de la extensión: [engramo.mcpb](https://github.com/engramo-developer/engramo-mcp/releases/latest/download/engramo.mcpb).
2. Abre Claude Desktop.
3. Haz doble clic en el archivo `engramo.mcpb` descargado (o arrástralo a la ventana de Claude Desktop).
4. Haz clic en **Install** (Instalar).

   Claude muestra un aviso en rojo: la extensión tendrá acceso a tu ordenador y Anthropic no ha verificado
   al desarrollador. Claude muestra este aviso con todas las extensiones instaladas desde un archivo. Haz
   clic en **Install** para continuar.
5. Rellena los dos campos:
   - **EngrAmo API token:** pega tu token de EngrAmo.
   - **Gemini API keys for card audio:** pega tu clave de Gemini. (¿Varias claves? Sepáralas con comas.)
6. Haz clic en **Save** (Guardar) y comprueba que la extensión está **activada**.

Claude guarda las dos claves en el almacenamiento seguro de tu ordenador (el Llavero en Mac).

## Comprueba que funciona

Abre un **Chat** nuevo (la extensión no funciona en el modo Code de Claude) y pega:

```text
Comprueba mi conexión con EngrAmo: llama a get_server_version, list_tts_voices y get_subscription_usage.
Dime con palabras sencillas la versión del servidor, si el audio está listo (y la voz por defecto), mi
plan y cuántas tarjetas puedo crear todavía. Si alguna llamada falla, explica por qué y cómo arreglarlo.
```

Cuando Claude pida permiso para usar EngrAmo, haz clic en **Allow** (Permitir). Debería decir que el audio
está listo.

Después prueba:

```text
Crea un catálogo de EngrAmo "Inglés para entrevistas de trabajo" con 10 tarjetas de frases útiles. Pon la
frase en inglés en el anverso y la traducción al español con un ejemplo en el reverso, y añade audio a
cada tarjeta.
```

## Conviene saber

- **¿Ya conectaste EngrAmo en Claude en el navegador?** Esa conexión no tiene audio. Cuando quieras audio,
  empieza tu mensaje con «Usando la extensión de EngrAmo…» o desactiva el conector web en Claude Desktop
  en **Settings → Connectors**.
- **Cambiar las claves más tarde:** **Settings → Extensions → EngrAmo → Configure** (Configurar).
- **Actualizaciones:** la extensión no se actualiza sola. Para actualizar, descarga de nuevo
  `engramo.mcpb` e instálalo igual que antes.
- **El audio** se crea para el anverso de cada tarjeta, hasta 20 tarjetas a la vez. Claude divide los mazos
  más grandes en tandas por su cuenta.

## Solución de problemas

| Problema | Qué hacer |
|---|---|
| Claude dice que el audio no está disponible | Revisa la clave de Gemini en **Settings → Extensions → EngrAmo → Configure** y luego cierra Claude Desktop por completo (⌘Q en Mac) y vuelve a abrirlo. |
| «Invalid token» o EngrAmo no responde | Genera un nuevo token de EngrAmo ([claves](keys.md)) y pégalo en **Configure**. |
| El audio se detiene con un mensaje de cuota o «rate limit» | Se agotó tu cuota diaria gratuita de Gemini. Inténtalo mañana o añade otra clave ([claves](keys.md)). |
