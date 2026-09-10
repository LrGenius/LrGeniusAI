//
//  CaptureOnePlugins.h
//
//  RECONSTRUCTED interface for Capture One's plugin SDK.
//
//  This file is NOT from Phase One. It was reconstructed by reading the Objective-C
//  runtime metadata that ships inside two binaries already present on this machine:
//
//    1. /Applications/Capture One.app/Contents/Frameworks/CaptureOnePlugins.framework
//       (the framework binary — exported classes, properties and their type encodings)
//    2. ~/Library/Application Support/Capture One/Plug-ins/COHeliconFocusPlugin.coplugin
//       (a real third-party plugin — Objective-C protocols are emitted into whichever
//        binary *uses* them, so this plugin's binary still carries the full text of
//        COOpenWithPlugin, COEditingPlugin, COSettings, COFileHandling,
//        COVariantProcessing and COActionSettings, including method type encodings)
//
//  Everything below that is marked "verified" was read out of those binaries with
//  `nm`, `otool -oV` and `strings`; the selector names and the ObjC type encodings
//  are exact. Everything marked SDK-UNVERIFIED could not be recovered from metadata
//  (enum *values* and *names* are compile-time constants and leave no trace in a
//  binary) and is a documented guess.
//
//  Replace this file with the official headers as soon as the real SDK is available.
//  See Headers/README.md for the symbol-by-symbol comparison checklist.
//
//  Verified against Capture One 16.8.5.30.
//

#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN

@class COPluginTask;
@class COPluginAction;
@class COSettingsItem;

#pragma mark - Constants (verified: exported symbols of CaptureOnePlugins)

/// Keys found inside COPluginTask.environment.
extern NSString *const COPluginTaskDestinationFolder;
extern NSString *const COPluginTaskTemporaryFolder;
extern NSString *const COPluginTaskExecutingDocumentType;

/// Values for COPluginTaskExecutingDocumentType.
extern NSString *const COPluginTaskDocumentTypeCatalog;
extern NSString *const COPluginTaskDocumentTypeSession;

/// Declares which file formats an action accepts.
extern NSString *const COSupportedFileFormatsKey;

extern NSString *const COPluginActionDisplayNameKey;
extern NSString *const COPluginActionIdentifierKey;
extern NSString *const COFileHandlingPluginTaskFilesKey;

#pragma mark - Enums (SDK-UNVERIFIED: values reconstructed by inference)

/// SDK-UNVERIFIED: the real type name and its cases are unknown.
///
/// Evidence for the numbering: COHeliconFocusPlugin's implementation of
/// -openWithActionsWithFileInfo:pluginRole:error: begins with
/// `testq %r14,%r14 ; jne <return nil>` — i.e. it returns no actions unless
/// pluginRole == 0. Helicon Focus is an "Open With" plugin, so 0 is the
/// Open-With role. The framework also ships COPluginActionPublishResult and
/// COPluginActionColorProfilingResult, which strongly implies that the same
/// callback is reused to enumerate publish and colour-profiling actions under
/// other role values. The 1 and 2 assignments below are ordering guesses.
typedef NS_ENUM(NSUInteger, COPluginRole) {
    COPluginRoleOpenWith        = 0,
    COPluginRolePublish         = 1,  // SDK-UNVERIFIED
    COPluginRoleColorProfiling  = 2,  // SDK-UNVERIFIED
};

/// SDK-UNVERIFIED: opaque event code delivered to -handleEvent:forSettingsItem:error:callback:.
/// Only the parameter's ObjC type (NSUInteger) is verified.
typedef NSUInteger COSettingsEvent;

/// SDK-UNVERIFIED: opaque code handed *back* through the settings callback block to tell
/// Capture One what to do next (e.g. re-read the settings tree). Only its type is verified.
typedef NSUInteger COSettingsCallbackAction;

#pragma mark - Progress (verified encoding, SDK-UNVERIFIED typedef name)

/// Verified from the type encoding of -startOpenWithTask:error:progress: :
///   @?<v@?@"COPluginTask"QQ@"NSString">
/// i.e. void (^)(COPluginTask *, NSUInteger, NSUInteger, NSString *).
///
/// SDK-UNVERIFIED: the argument *meaning*. The (NSUInteger, NSUInteger) pair is
/// assumed to be (completedUnits, totalUnits) — that ordering matches Capture One's
/// own AppleScript properties "progress completed units" / "progress total units".
typedef void (^COPluginTaskProgress)(COPluginTask *task,
                                     NSUInteger completedUnits,
                                     NSUInteger totalUnits,
                                     NSString *_Nullable message);

#pragma mark - Actions and results (verified)

@interface COPluginAction : NSObject <NSCopying, NSSecureCoding>
@property (nonatomic, copy) NSString *displayName;
@property (nonatomic, copy, nullable) NSString *identifier;
@property (nonatomic, strong, nullable) NSImage *image;
- (instancetype)initWithDisplayName:(NSString *)displayName;
- (instancetype)initWithDisplayName:(NSString *)displayName image:(nullable NSImage *)image;
+ (instancetype)pluginActionWithDisplayName:(NSString *)displayName;
- (BOOL)isEqualToPluginAction:(COPluginAction *)other;
@end

@interface COPluginActionResult : NSObject <NSCopying, NSSecureCoding>
@property (nonatomic) BOOL suppressNotification;
@end

/// Verified: `status` is a BOOL (property attribute string "TB,N,V_status").
@interface COPluginActionOpenWithResult : COPluginActionResult
@property (nonatomic) BOOL status;
- (instancetype)initWithStatus:(BOOL)status;
@end

@interface COPluginActionImageResult : COPluginActionResult
@property (nonatomic, copy) NSArray<NSString *> *images;
- (instancetype)initWithImages:(NSArray<NSString *> *)images;
@end

@interface COPluginActionPublishResult : COPluginActionResult
@property (nonatomic, copy, nullable) NSString *URL;
@property (nonatomic, copy, nullable) NSString *message;
- (instancetype)initWithURL:(nullable NSString *)url message:(nullable NSString *)message;
@end

@interface COPluginActionColorProfilingResult : COPluginActionResult
@property (nonatomic, copy) NSArray<NSString *> *colorProfiles;
- (instancetype)initWithColorProfiles:(NSArray<NSString *> *)colorProfiles;
@end

#pragma mark - Tasks (verified)

@interface COPluginTask : NSObject <NSCopying, NSSecureCoding>
@property (nonatomic, readonly) NSUUID *uuid;
@property (nonatomic, readonly) COPluginAction *action;
@property (nonatomic) BOOL cancelled;
@property (nonatomic, copy, nullable) NSDictionary *settings;
@property (nonatomic, copy, nullable) NSDictionary *environment;
- (instancetype)initWithAction:(COPluginAction *)action;
- (instancetype)initWithAction:(COPluginAction *)action settings:(nullable NSDictionary *)settings;
@end

@interface COFileHandlingPluginTask : COPluginTask
@property (nonatomic, copy) NSArray<NSString *> *files;
- (instancetype)initWithAction:(COPluginAction *)action files:(NSArray<NSString *> *)files;
@end

#pragma mark - Settings model (verified)

@interface COSettingsBase : NSObject <NSCopying, NSSecureCoding>
@property (nonatomic, copy, nullable) NSString *identifier;
@property (nonatomic, copy, nullable) NSString *title;
@property (nonatomic, copy, nullable) NSString *informativeText;
- (instancetype)initWithIdentifier:(nullable NSString *)identifier title:(nullable NSString *)title;
@end

@interface COSettingsElement : COSettingsBase
@end

@interface COSettingsItem : COSettingsElement
@end

/// A titled group of *elements* (the top level of a settings tree).
@interface COSettingsElementsGroup : COSettingsElement
@property (nonatomic, copy) NSArray<COSettingsElement *> *elements;
@end

/// A titled group of *items* (one row cluster inside a group).
@interface COSettingsItemsGroup : COSettingsElement
@property (nonatomic, copy) NSArray<COSettingsItem *> *items;
@end

@interface COSettingsLabelItem : COSettingsItem
@property (nonatomic, copy, nullable) NSString *value;
@end

@interface COSettingsBoolItem : COSettingsItem
@property (nonatomic) BOOL value;
@end

@interface COSettingsTextItem : COSettingsItem
@property (nonatomic, copy, nullable) NSString *value;
@property (nonatomic) BOOL secure;
@end

/// Verified: `context` is id<NSSecureCoding>. It is echoed back to
/// -handleEvent:forSettingsItem:error:callback: so a button can identify itself
/// without the plugin storing any state.
@interface COSettingsButtonItem : COSettingsItem
@property (nonatomic, strong, nullable) id<NSSecureCoding> context;
@end

@interface COSettingsFileItem : COSettingsItem
@property (nonatomic, copy, nullable) NSString *value;
@property (nonatomic) BOOL canChooseFiles;
@property (nonatomic) BOOL canChooseDirectories;
@property (nonatomic) BOOL allowsMultipleSelection;
@property (nonatomic, copy, nullable) NSArray<NSString *> *allowedFileTypes;
@property (nonatomic, copy, nullable) NSString *directoryURL;
@property (nonatomic, copy, nullable) NSString *placeholder;
@end

@interface COSettingsListOption : NSObject <NSCopying, NSSecureCoding>
@property (nonatomic, strong, nullable) id<NSSecureCoding> value;
@property (nonatomic, copy, nullable) NSString *title;
@property (nonatomic, strong, nullable) NSImage *image;
+ (instancetype)settingsListOptionWithValue:(nullable id<NSSecureCoding>)value title:(nullable NSString *)title;
+ (instancetype)separator;
@end

@interface COSettingsListItem : COSettingsItem
@property (nonatomic, strong, nullable) id<NSSecureCoding> value;
@property (nonatomic, copy, nullable) NSArray<COSettingsListOption *> *options;
@end

@interface COSettingsMultipleListItem : COSettingsItem
@property (nonatomic, copy, nullable) NSArray *value;
@property (nonatomic, copy, nullable) NSArray<COSettingsListOption *> *options;
@property (nonatomic) BOOL allowsFiltering;
@property (nonatomic, copy, nullable) NSString *filteringTextPlaceholder;
@property (nonatomic) NSUInteger visibleRows;
@property (nonatomic) BOOL showsSelectDeselectAll;
@end

#pragma mark - Protocols (verified: recovered from COHeliconFocusPlugin's protocol refs)

/// Turns a chosen action plus a concrete file list into one or more units of work.
@protocol COFileHandling <NSObject>
- (nullable NSArray<COPluginTask *> *)tasksForAction:(COPluginAction *)action
                                            forFiles:(NSArray<NSString *> *)files
                                               error:(NSError **)error;
@end

/// Implementing this protocol is what makes Capture One hand a plugin *processed*
/// variants instead of the original files on disk. Do not adopt it if you want RAWs.
@protocol COVariantProcessing <NSObject>
- (nullable NSDictionary *)processingSettingsForAction:(COPluginAction *)action error:(NSError **)error;
- (NSUInteger)processingSettingsVisibilityForAction:(COPluginAction *)action;
@end

@protocol COActionSettings <NSObject>
- (nullable NSArray<COSettingsElement *> *)settingsForAction:(COPluginAction *)action
                                                    settings:(nullable NSDictionary *)settings
                                                       error:(NSError **)error;
- (BOOL)didUpdateValue:(nullable id<NSSecureCoding>)value
            forSetting:(NSString *)settingIdentifier
                action:(COPluginAction *)action
              settings:(nullable NSDictionary *)settings
        callbackAction:(COSettingsCallbackAction *)callbackAction
                 error:(NSError **)error;
- (BOOL)validateSettings:(nullable NSDictionary *)settings
              forAction:(COPluginAction *)action
                  error:(NSError **)error;
@end

@protocol COOpenWithPlugin <COFileHandling>
- (nullable NSArray<COPluginAction *> *)openWithActionsWithFileInfo:(nullable NSDictionary *)fileInfo
                                                        pluginRole:(COPluginRole)pluginRole
                                                             error:(NSError **)error;
- (nullable COPluginActionOpenWithResult *)startOpenWithTask:(COFileHandlingPluginTask *)task
                                                       error:(NSError **)error
                                                    progress:(nullable COPluginTaskProgress)progress;
@end

@protocol COEditingPlugin <COFileHandling, COVariantProcessing, COActionSettings>
- (nullable NSArray<COPluginAction *> *)editingActionsWithFileInfo:(nullable NSDictionary *)fileInfo
                                                             error:(NSError **)error;
- (nullable COPluginActionImageResult *)startEditingTask:(COFileHandlingPluginTask *)task
                                                    error:(NSError **)error
                                                 progress:(nullable COPluginTaskProgress)progress;
@end

/// The plugin-wide (not per-action) preferences pane.
@protocol COSettings <NSObject>
- (nullable NSArray<COSettingsElement *> *)settingsWithError:(NSError **)error;
- (BOOL)didUpdateValue:(nullable id<NSSecureCoding>)value
            forSetting:(NSString *)settingIdentifier
                 error:(NSError **)error
              callback:(nullable void (^)(COSettingsCallbackAction action,
                                          id<NSCopying, NSSecureCoding> _Nullable value))callback;
- (BOOL)handleEvent:(COSettingsEvent)event
    forSettingsItem:(COSettingsItem *)item
              error:(NSError **)error
           callback:(nullable void (^)(COSettingsCallbackAction action,
                                       id<NSCopying, NSSecureCoding> _Nullable value))callback;
@end

#pragma mark - Base class (verified: exported, and carries no methods of its own)

/// COPluginBase exports no instance methods in 16.8.5.30 — the host discovers what a
/// plugin can do purely through protocol conformance / -respondsToSelector:.
///
/// The two selectors below are NOT on COPluginBase in the framework's metadata, yet
/// COHeliconFocusPlugin implements both, so they are host-invoked hooks declared in
/// the official header. Signatures are verified from Helicon's own type encodings
/// (`@24@0:8@16` and `c32@0:8@16@24`); the *semantics* are inferred.
@interface COPluginBase : NSObject

/// SDK-UNVERIFIED semantics. Helicon uses the returned string as a defaults-key prefix
/// for per-action persisted settings.
- (nullable NSString *)keepSettingsForActionKey:(COPluginAction *)action;

/// SDK-UNVERIFIED semantics. Only meaningful for COVariantProcessing plugins: whether the
/// processed files Capture One rendered should be kept after the task finishes.
- (BOOL)keepProcessedFilesForAction:(COPluginAction *)action settings:(nullable NSDictionary *)settings;

@end

NS_ASSUME_NONNULL_END
