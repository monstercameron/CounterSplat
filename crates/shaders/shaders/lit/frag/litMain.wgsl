// Extracted from PlayCanvas engine (MIT), commit 209ce5752527ab5050ec9632ca6dc1272542b70d.
// Source: src/scene/shader-lib/wgsl/chunks/lit/frag/litMain.js
#include "varyingsPS"
#include "litUserDeclarationPS"
#include "frontendDeclPS"
#include "outlineDeclarationPS"

#ifdef CLIP_PLANES
    #include "clipPlanesPS"
#endif

#if defined(PICK_PASS) || defined(PREPASS_PASS)

    #include "frontendCodePS"
    #include "litUserCodePS"
    #include "litOtherMainPS"

#elif defined(SHADOW_PASS)

    #include "frontendCodePS"
    #include "litUserCodePS"
    #include "litShadowMainPS"

#else // FORWARD_PASS

    #include "litForwardDeclarationPS"
    #include "litForwardPreCodePS"
    #include "frontendCodePS"
    #include "litForwardPostCodePS"
    #include "litForwardBackendPS"
    #include "litUserCodePS"
    #include "litForwardMainPS"

#endif

