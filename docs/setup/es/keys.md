# Consigue tus claves (para el audio)

[← Todas las guías](README.md)

Las configuraciones con audio ([Claude Desktop](claude-desktop-audio.md), [Antigravity](antigravity-audio.md))
necesitan dos claves. Una clave es un texto largo, parecido a una contraseña, que permite a una aplicación
actuar en tu nombre. Tardas unos 5 minutos.

> **Mantén tus claves en privado.** No las pegues en un chat, un correo ni una captura de pantalla.
> Cualquiera que tenga una clave puede usar tu cuenta. Si una clave se filtra, bórrala y crea otra.

## 1. Token de API de EngrAmo

Permite que la aplicación de tu ordenador use tu cuenta de EngrAmo.

1. Abre <https://study.engramo.app> e inicia sesión.
2. Haz clic en el **icono de persona** (arriba a la derecha) → **API token**.
3. Haz clic en **Generate** (Generar).
4. Copia el token (empieza por `engramo_`) y guárdalo en un lugar seguro, por ejemplo un gestor de
   contraseñas. Puede que después no puedas volver a verlo. Si lo pierdes, genera uno nuevo.

## 2. Clave de API de Gemini (gratis)

Permite que la aplicación cree el audio de las tarjetas con las voces de Google. Es gratis, y el audio usa
tu propia cuota de Google, no tu plan de EngrAmo.

1. Abre <https://aistudio.google.com/apikey> e inicia sesión con una cuenta de Google.
2. Si te lo pide, acepta las condiciones.
3. Haz clic en **Create API key** (Crear clave de API). Si te pide elegir un proyecto, elige cualquiera (o
   deja que cree uno).
4. Copia la clave (normalmente empieza por `AIza`) y guárdala en un lugar seguro.

**¿Vas a crear mucho audio?** La cuota gratuita tiene un límite diario. Puedes crear claves en varios
proyectos de Google distintos e introducirlas todas separadas por comas (`clave1,clave2`). EngrAmo pasa a
la siguiente clave cuando una se agota.

## Siguiente paso

- [Claude Desktop con audio](claude-desktop-audio.md)
- [Antigravity con audio](antigravity-audio.md)
