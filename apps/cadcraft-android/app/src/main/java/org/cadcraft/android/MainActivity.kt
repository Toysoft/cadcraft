package org.cadcraft.android

import android.app.Activity
import android.content.pm.ApplicationInfo
import android.os.Bundle
import android.util.Log
import android.webkit.ConsoleMessage
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import androidx.webkit.WebViewAssetLoader
import androidx.webkit.WebViewClientCompat

class MainActivity : Activity() {

    private lateinit var webView: WebView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // CADCRAFT_LIFECYCLE_DIAGNOSTICS
        Log.d(
            "CADCraftWeb",
            "Activity onCreate: orientation=${resources.configuration.orientation}"
        )


        window.setSoftInputMode(
            android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
        )

        WebView.setWebContentsDebuggingEnabled(
            (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
        )

        val assetLoader = WebViewAssetLoader.Builder()
            .addPathHandler(
                "/assets/",
                WebViewAssetLoader.AssetsPathHandler(this)
            )
            .build()

        webView = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true

            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.javaScriptCanOpenWindowsAutomatically = false

            webChromeClient = object : WebChromeClient() {
                override fun onConsoleMessage(
                    message: ConsoleMessage
                ): Boolean {
                    Log.d(
                        "CADCraftWeb",
                        "${message.message()} " +
                        "(${message.sourceId()}:${message.lineNumber()})"
                    )
                    return true
                }
            }

            webViewClient = object : WebViewClientCompat() {
    override fun onRenderProcessGone(
        view: android.webkit.WebView,
        detail: android.webkit.RenderProcessGoneDetail
    ): Boolean {
        android.util.Log.e(
            "CADCraftWeb",
            "WebView renderer terminated. " +
                "didCrash=${detail.didCrash()}, " +
                "priority=${detail.rendererPriorityAtExit()}"
        )
        return false
    }


                override fun onPageStarted(
                    view: WebView,
                    url: String,
                    favicon: android.graphics.Bitmap?
                ) {
                    super.onPageStarted(view, url, favicon)
                    Log.d("CADCraftWeb", "Page started: $url")
                }

                override fun shouldInterceptRequest(
                    view: WebView,
                    request: WebResourceRequest
                ): WebResourceResponse? {
                    val response = assetLoader.shouldInterceptRequest(request.url)

                    if (request.url.path?.endsWith(".wasm") == true) {
                        response?.mimeType = "application/wasm"
                    }

                    return response
                }

                override fun onPageFinished(
                    view: WebView,
                    url: String
                ) {
                    super.onPageFinished(view, url)
                    Log.d("CADCraftWeb", "Page loaded: $url")
                }
            }
        }

        setContentView(webView)

        webView.loadUrl(
            "https://appassets.androidplatform.net/assets/web/index.html?mobile=1&cpu"
        )
    }


    override fun onConfigurationChanged(
        newConfig: android.content.res.Configuration
    ) {
        super.onConfigurationChanged(newConfig)
        Log.d(
            "CADCraftWeb",
            "Configuration changed: orientation=${newConfig.orientation}"
        )
    }

    override fun onDestroy() {
        Log.d("CADCraftWeb", "Activity onDestroy")
        webView.destroy()
        super.onDestroy()
    }
}
