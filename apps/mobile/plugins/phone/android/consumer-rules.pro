# Keep the plugin class: Tauri finds it by name.
-keep class com.anytask.phone.** { *; }
# The plugin reads androidx's ActivityResult by name (it cannot compile against androidx.activity): without this,
# the release build's shrinker renames or inlines the two getters and every file pick and voice result crashes.
-keep class androidx.activity.result.ActivityResult { public int getResultCode(); public android.content.Intent getData(); }
