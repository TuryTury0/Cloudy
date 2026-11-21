// AnyTask phone access for the paired PC: status (battery, device), recent notifications, SMS inbox, launching
// apps and links. Notifications need the user to allow "notification access"; SMS needs the READ_SMS permission.
// Called from the app's Rust side when the PC's assistant asks (the user asked it something about the phone).
// Also for the phone's own screens: picking a file to send to the PC, saving a file from the PC in Downloads,
// ringing the phone so it can be found, and the reason of the last crash (written when the app crashes, read on
// the next start, so a crash is explained instead of guessed).

package com.anytask.phone

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.ContentValues
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.Ringtone
import android.media.RingtoneManager
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.AlarmClock
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.provider.Telephony
import android.speech.RecognizerIntent
import android.speech.tts.TextToSpeech
import java.util.Locale
import android.webkit.MimeTypeMap
import android.webkit.WebView
import app.tauri.PermissionState
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class LimitArgs {
    var limit: Int = 20
}

@InvokeArg
class NameArgs {
    var name: String = ""
}

@InvokeArg
class UrlArgs {
    var url: String = ""
}

@InvokeArg
class SaveArgs {
    var path: String = ""
    var name: String = ""
}

@InvokeArg
class OpenArgs {
    var uri: String = ""
}

@InvokeArg
class ListenArgs {
    var lang: String = "fa-IR"
}

@InvokeArg
class SpeakArgs {
    var text: String = ""
}

@InvokeArg
class AlarmArgs {
    var hour: Int = -1
    var minute: Int = 0
    var label: String = ""
}

@InvokeArg
class TimerArgs {
    var seconds: Int = 0
    var label: String = ""
}

@InvokeArg
class DialArgs {
    var number: String = ""
    var text: String = ""
}

@InvokeArg
class PlaceArgs {
    var destination: String = ""
}

/** Where the reason of a crash is kept until the next start. */
private const val CRASH_FILE = "anytask-last-crash.txt"

private const val SMS = "sms"

@TauriPlugin(permissions = [Permission(strings = [Manifest.permission.READ_SMS], alias = SMS)])
class PhonePlugin(private val activity: Activity) : Plugin(activity) {

    private var ringing: Ringtone? = null
    /** The alarm volume before ringing, put back when the ringing stops (it is also the user's morning alarm). */
    private var alarmVolume: Int? = null

    private fun endRing() {
        ringing?.stop()
        ringing = null
        val before = alarmVolume ?: return
        alarmVolume = null
        try {
            activity.getSystemService(AudioManager::class.java)?.setStreamVolume(AudioManager.STREAM_ALARM, before, 0)
        } catch (_: SecurityException) {
        }
    }

    /** A file name from the PC without folders or characters Android refuses. */
    private fun safeName(name: String): String {
        val base = name.substringAfterLast('/').substringAfterLast('\\').replace(Regex("[\\x00-\\x1f:*?\"<>|]"), "_").trim().trimStart('.')
        return base.ifBlank { "file" }.take(150)
    }

    /** Writes any crash's reason to a file before Android ends the app; the next start shows and reports it. */
    override fun load(webView: WebView) {
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            try {
                val trace = android.util.Log.getStackTraceString(error)
                java.io.File(activity.filesDir, CRASH_FILE).writeText("${System.currentTimeMillis()} ${Build.MANUFACTURER} ${Build.MODEL} Android ${Build.VERSION.RELEASE}\n$trace".take(20000))
            } catch (_: Throwable) {
            }
            previous?.uncaughtException(thread, error)
        }
    }

    /** The reason of the last crash, once (the file is removed after reading); empty when there was none. */
    @Command
    fun lastCrash(invoke: Invoke) {
        val file = java.io.File(activity.filesDir, CRASH_FILE)
        val out = JSObject()
        out.put("text", if (file.exists()) file.readText() else "")
        file.delete()
        invoke.resolve(out)
    }

    /** Lets the user pick any file; it is copied into the app's cache so the Rust side can send it to the PC. */
    @Command
    fun pickFile(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*")
        startActivityForResult(invoke, intent, "filePicked")
    }

    // `result` is androidx's ActivityResult, which this library does not compile against (it is not exported by
    // tauri-android, and no new library can be fetched here): its two values are read by name.
    /** The result code and data of androidx's ActivityResult, read by name; null when they cannot be read. */
    private fun activityResult(result: Any): Pair<Int, Intent?>? = try {
        val code = result.javaClass.getMethod("getResultCode").invoke(result) as Int
        val data = result.javaClass.getMethod("getData").invoke(result) as Intent?
        code to data
    } catch (_: Throwable) {
        null
    }

    @ActivityCallback
    fun filePicked(invoke: Invoke, result: Any) {
        // Never let a reading failure end the app (Tauri calls this without a catch): the promise is answered.
        val (code, data) = activityResult(result) ?: run {
            invoke.reject("The file could not be read from the picker.")
            return
        }
        val uri: Uri? = data?.data
        if (code != Activity.RESULT_OK || uri == null) {
            invoke.reject("No file was chosen.")
            return
        }
        Thread {
            try {
                var name = "file"
                activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                    if (c.moveToFirst()) name = c.getString(0) ?: name
                }
                val folder = java.io.File(activity.cacheDir, "upload").apply { mkdirs() }
                folder.listFiles()?.forEach { it.delete() }
                val copy = java.io.File(folder, name.replace(Regex("[\\\\/:*?\"<>|]"), "_"))
                activity.contentResolver.openInputStream(uri)?.use { input -> copy.outputStream().use { input.copyTo(it) } }
                val out = JSObject()
                out.put("path", copy.absolutePath)
                out.put("name", name)
                out.put("size", copy.length())
                invoke.resolve(out)
            } catch (e: Exception) {
                invoke.reject("Could not read that file: ${e.message}")
            }
        }.start()
    }

    /** Whether AnyTask is the app on the phone's screen: only then may it open another app (Android 10+ blocks
     * starts from the background without any error, so an alarm would be "set" while nothing happened). */
    @Volatile
    private var visible = true

    override fun onResume() {
        visible = true
    }

    override fun onPause() {
        visible = false
    }

    private val inBackground = "Open AnyTask on the phone first: Android does not let it open other apps from the background."

    /** Starts another app's screen for the user: null when it was asked, else why not (in the background, or no app
     * on the phone handles it). */
    private fun startFor(intent: Intent, none: String): String? {
        if (!visible) return inBackground
        return try {
            activity.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            null
        } catch (_: Exception) {
            none
        }
    }

    private fun answer(invoke: Invoke, problem: String?, text: String) {
        if (problem != null) invoke.reject(problem) else invoke.resolve(JSObject().apply { put("done", text) })
    }

    /** A phone number with ASCII digits: speech and Persian keyboards give ۰-۹, which some dialers do not read. */
    private fun asciiNumber(text: String, keep: String): String =
        text.mapNotNull { c -> Character.digit(c, 10).takeIf { it >= 0 }?.let { '0' + it } ?: c.takeIf { it in keep } }.joinToString("")

    /** Sets an alarm in the phone's clock app (hour 0-23). */
    @Command
    fun setAlarm(invoke: Invoke) {
        val args = invoke.parseArgs(AlarmArgs::class.java)
        if (args.hour !in 0..23 || args.minute !in 0..59) {
            invoke.reject("The alarm needs an hour 0-23 and minutes 0-59.")
            return
        }
        val intent = Intent(AlarmClock.ACTION_SET_ALARM)
            .putExtra(AlarmClock.EXTRA_HOUR, args.hour)
            .putExtra(AlarmClock.EXTRA_MINUTES, args.minute)
            .putExtra(AlarmClock.EXTRA_SKIP_UI, true)
        if (args.label.isNotBlank()) intent.putExtra(AlarmClock.EXTRA_MESSAGE, args.label)
        // "Asked": a choice between two clock apps, or one that shows its own screen, still needs the user.
        answer(invoke, startFor(intent, "The phone's clock app does not take alarms from other apps."), "asked the phone's clock app to set an alarm for %02d:%02d".format(args.hour, args.minute))
    }

    /** Starts a timer in the phone's clock app. */
    @Command
    fun setTimer(invoke: Invoke) {
        val args = invoke.parseArgs(TimerArgs::class.java)
        if (args.seconds !in 1..86_400) {
            invoke.reject("The timer needs 1 second to 24 hours.")
            return
        }
        val intent = Intent(AlarmClock.ACTION_SET_TIMER).putExtra(AlarmClock.EXTRA_LENGTH, args.seconds).putExtra(AlarmClock.EXTRA_SKIP_UI, true)
        if (args.label.isNotBlank()) intent.putExtra(AlarmClock.EXTRA_MESSAGE, args.label)
        answer(invoke, startFor(intent, "The phone's clock app does not take timers from other apps."), "asked the phone's clock app to start a ${args.seconds} s timer")
    }

    /** Opens the dialer with the number filled in; the user presses call (nothing is called by itself). */
    @Command
    fun dial(invoke: Invoke) {
        val number = asciiNumber(invoke.parseArgs(DialArgs::class.java).number, "+*#")
        if (number.isEmpty()) {
            invoke.reject("No phone number was given.")
            return
        }
        answer(invoke, startFor(Intent(Intent.ACTION_DIAL, Uri.parse("tel:" + Uri.encode(number))), "No dialer on this phone."), "the dialer shows $number; the user presses call")
    }

    /** Opens a new SMS with the number and text filled in; the user presses send (nothing is sent by itself). */
    @Command
    fun composeSms(invoke: Invoke) {
        val args = invoke.parseArgs(DialArgs::class.java)
        val number = asciiNumber(args.number, "+")
        val intent = Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:" + Uri.encode(number))).putExtra("sms_body", args.text)
        answer(invoke, startFor(intent, "No SMS app on this phone."), "the message is ready in the SMS app; the user presses send")
    }

    /** Opens the maps app with directions (or a search) for a place. */
    @Command
    fun navigate(invoke: Invoke) {
        val place = invoke.parseArgs(PlaceArgs::class.java).destination.trim()
        if (place.isEmpty()) {
            invoke.reject("No place was given.")
            return
        }
        answer(invoke, startFor(Intent(Intent.ACTION_VIEW, Uri.parse("geo:0,0?q=" + Uri.encode(place))), "No maps app on this phone."), "the maps app shows $place")
    }

    private var tts: TextToSpeech? = null
    private var ttsReady = false

    /** Reads a text aloud with the phone's own voice (Persian when the text is Persian and a Persian voice is
     * installed, else the phone's language). The first call starts the speech engine. */
    @Command
    fun speak(invoke: Invoke) {
        val text = invoke.parseArgs(SpeakArgs::class.java).text.trim().take(3500)
        if (text.isEmpty()) {
            invoke.resolve()
            return
        }
        val persian = text.any { it in '\u0600'..'\u06FF' }
        val say = {
            val engine = tts
            if (engine == null || !ttsReady) {
                invoke.reject("The phone has no speech voice.")
            } else {
                val wanted = if (persian) Locale("fa", "IR") else Locale.getDefault()
                val result = engine.setLanguage(wanted)
                if (persian && (result == TextToSpeech.LANG_MISSING_DATA || result == TextToSpeech.LANG_NOT_SUPPORTED)) {
                    invoke.reject("No Persian voice is installed on the phone (Settings > Text-to-speech).")
                } else {
                    engine.speak(text, TextToSpeech.QUEUE_FLUSH, null, "anytask")
                    invoke.resolve()
                }
            }
        }
        if (tts != null) {
            say()
            return
        }
        tts = TextToSpeech(activity) { status ->
            ttsReady = status == TextToSpeech.SUCCESS
            Handler(Looper.getMainLooper()).post { say() }
        }
    }

    @Command
    fun stopSpeaking(invoke: Invoke) {
        tts?.stop()
        player?.release()
        player = null
        invoke.resolve()
    }

    private var player: MediaPlayer? = null

    /** Plays a sound file (the assistant's spoken answer, made on the PC) and removes it when done. */
    @Command
    fun playAudio(invoke: Invoke) {
        val path = invoke.parseArgs(SaveArgs::class.java).path
        Handler(Looper.getMainLooper()).post {
            try {
                player?.release()
                val file = java.io.File(path)
                val next = MediaPlayer()
                next.setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                next.setDataSource(path)
                next.setOnCompletionListener {
                    it.release()
                    if (player === it) player = null
                    file.delete()
                }
                next.prepare()
                next.start()
                player = next
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject("The answer's sound could not be played: ${e.message}")
            }
        }
    }

    /** Listens once with the phone's own speech recognition (Persian by default) and gives back what was said. */
    @Command
    fun listen(invoke: Invoke) {
        val lang = invoke.parseArgs(ListenArgs::class.java).lang.ifBlank { "fa-IR" }
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE, lang)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_PREFERENCE, lang)
            .putExtra(RecognizerIntent.EXTRA_PROMPT, if (lang.startsWith("fa")) "بگو چه کار کنم" else "Say what to do")
        if (intent.resolveActivity(activity.packageManager) == null) {
            invoke.reject("This phone has no speech recognition (install or update the Google app).")
            return
        }
        startActivityForResult(invoke, intent, "heard")
    }

    // `result` is androidx's ActivityResult, read by name (see filePicked).
    @ActivityCallback
    fun heard(invoke: Invoke, result: Any) {
        val (code, data) = activityResult(result) ?: run {
            invoke.reject("Nothing was heard.")
            return
        }
        @Suppress("DEPRECATION")
        when (code) {
            // Persian recognition works online only: say so instead of a silent nothing.
            RecognizerIntent.RESULT_NETWORK_ERROR, RecognizerIntent.RESULT_SERVER_ERROR -> {
                invoke.reject("Speech recognition needs the internet (a VPN may be needed).")
                return
            }
            RecognizerIntent.RESULT_AUDIO_ERROR -> {
                invoke.reject("The microphone could not be used.")
                return
            }
        }
        val text = data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)?.firstOrNull()?.trim().orEmpty()
        if (code != Activity.RESULT_OK || text.isEmpty()) {
            invoke.reject("Nothing was heard.")
            return
        }
        val out = JSObject()
        out.put("text", text)
        invoke.resolve(out)
    }

    /** Saves a file the PC sent (a path in the app's storage) into Downloads/AnyTask, where the Files app shows it. */
    @Command
    fun saveFile(invoke: Invoke) {
        val args = invoke.parseArgs(SaveArgs::class.java)
        val source = java.io.File(args.path)
        if (!source.exists()) {
            invoke.reject("The file is missing.")
            return
        }
        Thread {
            try {
                val name = safeName(args.name.ifBlank { source.name })
                val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"
                val out = JSObject()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    val values = ContentValues().apply {
                        put(MediaStore.Downloads.DISPLAY_NAME, name)
                        put(MediaStore.Downloads.MIME_TYPE, mime)
                        put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/AnyTask")
                        // Hidden until complete: a copy that fails leaves no broken entry in Downloads.
                        put(MediaStore.Downloads.IS_PENDING, 1)
                    }
                    val resolver = activity.contentResolver
                    val target = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: throw Exception("Downloads refused the file")
                    try {
                        resolver.openOutputStream(target)?.use { o -> source.inputStream().use { it.copyTo(o) } } ?: throw Exception("Downloads refused the file")
                        resolver.update(target, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
                    } catch (e: Exception) {
                        resolver.delete(target, null, null)
                        throw e
                    }
                    out.put("uri", target.toString())
                } else {
                    // Android 7-9: the app's own Downloads folder needs no permission (Files apps show it under
                    // Android/data); never over an existing file.
                    val folder = java.io.File(activity.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: activity.filesDir, "AnyTask").apply { mkdirs() }
                    var target = java.io.File(folder, name)
                    var n = 2
                    while (target.exists()) {
                        target = java.io.File(folder, name.substringBeforeLast('.', name) + " ($n)" + (if (name.contains('.')) "." + name.substringAfterLast('.') else ""))
                        n++
                    }
                    source.copyTo(target)
                    out.put("uri", "")
                    out.put("path", target.absolutePath)
                }
                source.delete()
                out.put("name", name)
                out.put("mime", mime)
                invoke.resolve(out)
            } catch (e: Exception) {
                invoke.reject("Could not save it in Downloads: ${e.message}")
            }
        }.start()
    }

    /** Opens a saved file in the app that shows its type. */
    @Command
    fun openFile(invoke: Invoke) {
        val text = invoke.parseArgs(OpenArgs::class.java).uri
        // Only content:// links can be handed to other apps (a file:// one makes Android end the app).
        if (!text.startsWith("content://")) {
            invoke.reject("Open it from the Files app (Download/AnyTask).")
            return
        }
        val uri = Uri.parse(text)
        try {
            val mime = activity.contentResolver.getType(uri) ?: "*/*"
            activity.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK))
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("No app on the phone opens this file.")
        }
    }

    /** Rings the phone loudly for 30 seconds (the alarm sound, even in silent mode) so the user can find it. */
    @Command
    fun ring(invoke: Invoke) {
        Handler(Looper.getMainLooper()).post {
            endRing()
            val uri = RingtoneManager.getActualDefaultRingtoneUri(activity, RingtoneManager.TYPE_ALARM)
                ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE)
            val tone = RingtoneManager.getRingtone(activity, uri)
            if (tone == null) {
                invoke.reject("The phone has no sound to ring with.")
                return@post
            }
            tone.audioAttributes = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ALARM).build()
            // Many alarm sounds play once: keep it going until stopped.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) tone.isLooping = true
            val audio = activity.getSystemService(AudioManager::class.java)
            alarmVolume = audio?.getStreamVolume(AudioManager.STREAM_ALARM)
            // Do Not Disturb can refuse a volume change; the alarm still sounds at its own volume.
            try {
                audio?.setStreamVolume(AudioManager.STREAM_ALARM, audio.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0)
            } catch (_: SecurityException) {
            }
            tone.play()
            ringing = tone
            Handler(Looper.getMainLooper()).postDelayed({ if (ringing === tone) endRing() }, 30_000)
            val out = JSObject()
            out.put("ringing", true)
            invoke.resolve(out)
        }
    }

    @Command
    fun stopRing(invoke: Invoke) {
        Handler(Looper.getMainLooper()).post {
            endRing()
            invoke.resolve()
        }
    }

    private fun notificationAccess(): Boolean {
        val enabled = Settings.Secure.getString(activity.contentResolver, "enabled_notification_listeners") ?: return false
        val me = ComponentName(activity, NotificationService::class.java).flattenToString()
        return enabled.split(":").any { it == me }
    }

    private fun smsAccess() = getPermissionState(SMS) == PermissionState.GRANTED

    @Command
    fun status(invoke: Invoke) {
        val battery = activity.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, 100) ?: 100
        val out = JSObject()
        out.put("device", "${Build.MANUFACTURER} ${Build.MODEL}")
        out.put("android", Build.VERSION.RELEASE)
        out.put("battery", if (level >= 0 && scale > 0) level * 100 / scale else -1)
        out.put("charging", (battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0) != 0)
        out.put("notificationAccess", notificationAccess())
        out.put("smsAccess", smsAccess())
        invoke.resolve(out)
    }

    @Command
    fun notifications(invoke: Invoke) {
        if (!notificationAccess()) {
            invoke.reject("Notification access is off; allow it in the AnyTask app on the phone.")
            return
        }
        val limit = invoke.parseArgs(LimitArgs::class.java).limit.coerceIn(1, 100)
        val list = JSArray()
        for (n in NotificationStore.recent(limit)) {
            val o = JSObject()
            o.put("app", n.app)
            o.put("title", n.title)
            o.put("text", n.text)
            o.put("time", n.time)
            list.put(o)
        }
        val out = JSObject()
        out.put("notifications", list)
        invoke.resolve(out)
    }

    @Command
    fun messages(invoke: Invoke) {
        if (!smsAccess()) {
            invoke.reject("SMS access is off; allow it in the AnyTask app on the phone.")
            return
        }
        val limit = invoke.parseArgs(LimitArgs::class.java).limit.coerceIn(1, 100)
        val list = JSArray()
        val columns = arrayOf(Telephony.Sms.ADDRESS, Telephony.Sms.BODY, Telephony.Sms.DATE, Telephony.Sms.READ)
        activity.contentResolver.query(Telephony.Sms.Inbox.CONTENT_URI, columns, null, null, "${Telephony.Sms.DATE} DESC")?.use { c ->
            var count = 0
            while (count < limit && c.moveToNext()) {
                val o = JSObject()
                o.put("from", c.getString(0) ?: "")
                o.put("text", (c.getString(1) ?: "").take(1000))
                o.put("time", c.getLong(2))
                o.put("read", c.getInt(3) == 1)
                list.put(o)
                count++
            }
        }
        val out = JSObject()
        out.put("messages", list)
        invoke.resolve(out)
    }

    private fun launchable(): List<Pair<String, String>> {
        val pm = activity.packageManager
        val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        return pm.queryIntentActivities(intent, 0)
            .map { it.loadLabel(pm).toString() to it.activityInfo.packageName }
            .distinctBy { it.second }
            .sortedBy { it.first.lowercase() }
    }

    @Command
    fun apps(invoke: Invoke) {
        val list = JSArray()
        for ((label, _) in launchable()) list.put(label)
        val out = JSObject()
        out.put("apps", list)
        invoke.resolve(out)
    }

    @Command
    fun openApp(invoke: Invoke) {
        val name = invoke.parseArgs(NameArgs::class.java).name.trim().lowercase()
        val apps = launchable()
        val match = apps.firstOrNull { it.first.lowercase() == name } ?: apps.firstOrNull { it.first.lowercase().contains(name) }
        val launch = match?.let { activity.packageManager.getLaunchIntentForPackage(it.second) }
        if (name.isEmpty() || launch == null) {
            invoke.reject("No app named \"$name\" on the phone.")
            return
        }
        startFor(launch, "That app could not be opened.")?.let {
            invoke.reject(it)
            return
        }
        val out = JSObject()
        out.put("opened", match.first)
        invoke.resolve(out)
    }

    @Command
    fun openUrl(invoke: Invoke) {
        val url = invoke.parseArgs(UrlArgs::class.java).url.trim()
        if (!url.startsWith("https://") && !url.startsWith("http://")) {
            invoke.reject("Only http(s) links can be opened.")
            return
        }
        startFor(Intent(Intent.ACTION_VIEW, Uri.parse(url)), "No app on the phone opens links.")?.let {
            invoke.reject(it)
            return
        }
        val out = JSObject()
        out.put("opened", url)
        invoke.resolve(out)
    }

    /** Opens Android's notification access screen, where the user turns AnyTask on. */
    @Command
    fun openNotificationAccess(invoke: Invoke) {
        activity.startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
        invoke.resolve()
    }

    @Command
    override fun requestPermissions(invoke: Invoke) {
        if (smsAccess()) {
            smsCallback(invoke)
        } else {
            requestPermissionForAlias(SMS, invoke, "smsCallback")
        }
    }

    @PermissionCallback
    fun smsCallback(invoke: Invoke) {
        val out = JSObject()
        out.put("sms", smsAccess())
        invoke.resolve(out)
    }
}
