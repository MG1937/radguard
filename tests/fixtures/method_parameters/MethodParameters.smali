.class public Lparameters/MethodParameters;
.super Ljava/lang/Object;

.method public constructor <init>(I)V
    .locals 1
    invoke-direct {p0}, Ljava/lang/Object;-><init>()V
    return-void
.end method

.method public native nativeMixed(IJDLjava/lang/String;[I)V
.end method

.method public static native staticNativeMixed(IJDLjava/lang/String;[I)V
.end method

.method public native nativeNoParameters()V
.end method

.method public static native staticNativeNoParameters()V
.end method

.method public instanceMixed(IJDLjava/lang/String;[I)D
    .locals 2
    return-wide p4
.end method

.method public static staticMixed(IJDLjava/lang/String;[I)Ljava/lang/String;
    .locals 1
    return-object p5
.end method

.method public concreteNoParameters()V
    .locals 0
    return-void
.end method
