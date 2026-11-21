// Keeps the phone's recent notifications (title, text, app) once the user allows notification access.
// Android starts this service by itself while access is on, so notifications are collected even when the
// AnyTask screen is closed. Ongoing ones (music players, downloads) are skipped.

package com.anytask.phone

import android.app.Notification
import android.content.Context
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification

class NotificationService : NotificationListenerService() {
    override fun onListenerConnected() {
        try {
            activeNotifications?.forEach { NotificationStore.add(this, it) }
        } catch (_: Exception) {
        }
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        NotificationStore.add(this, sbn)
    }
}

object NotificationStore {
    data class Item(val key: String, val app: String, val title: String, val text: String, val time: Long)

    private const val KEEP = 100
    private val items = ArrayDeque<Item>()

    @Synchronized
    fun add(context: Context, sbn: StatusBarNotification) {
        if (sbn.isOngoing) return
        val extras = sbn.notification.extras
        val title = extras.getCharSequence(Notification.EXTRA_TITLE)?.toString().orEmpty()
        val text = (extras.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: extras.getCharSequence(Notification.EXTRA_TEXT))?.toString().orEmpty()
        if (title.isBlank() && text.isBlank()) return
        val pm = context.packageManager
        val app = try {
            pm.getApplicationLabel(pm.getApplicationInfo(sbn.packageName, 0)).toString()
        } catch (_: Exception) {
            sbn.packageName
        }
        items.removeAll { it.key == sbn.key }
        items.addFirst(Item(sbn.key, app, title, text.take(500), sbn.postTime))
        while (items.size > KEEP) items.removeLast()
    }

    @Synchronized
    fun recent(limit: Int): List<Item> = items.take(limit)
}
