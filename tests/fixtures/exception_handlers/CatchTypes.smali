.class public Lexceptions/CatchTypes;
.super Ljava/lang/Object;

.method public static reuseForInt()I
    .locals 1
    :start
    invoke-static {}, Lexceptions/Support;->fail()V
    :end
    goto :done
    :first
    move-exception v0
    const/4 v0, 0x1
    return v0
    :second
    move-exception v0
    const/4 v0, 0x2
    return v0
    :done
    const/4 v0, 0x0
    return v0
    .catch Ljava/lang/IllegalArgumentException; {:start .. :end} :first
    .catch Ljava/lang/IllegalStateException; {:start .. :end} :second
.end method

.method public static reuseForReference()V
    .locals 1
    :start
    invoke-static {}, Lexceptions/Support;->fail()V
    :end
    goto :done
    :handler
    move-exception v0
    const-string v0, "caught"
    invoke-static {v0}, Lexceptions/Support;->consume(Ljava/lang/String;)V
    :done
    return-void
    .catch Ljava/lang/IllegalArgumentException; {:start .. :end} :handler
.end method

.method public static typedAndCatchAll()I
    .locals 1
    :start
    invoke-static {}, Lexceptions/Support;->fail()V
    :end
    goto :done
    :first
    move-exception v0
    const/4 v0, 0x1
    return v0
    :second
    move-exception v0
    const/4 v0, 0x2
    return v0
    :done
    const/4 v0, 0x0
    return v0
    .catch Ljava/lang/IllegalArgumentException; {:start .. :end} :first
    .catchall {:start .. :end} :second
.end method

.method public static multiCatch()I
    .locals 1
    :start
    invoke-static {}, Lexceptions/Support;->fail()V
    :end
    goto :done
    :handler
    move-exception v0
    const/4 v0, 0x3
    return v0
    :done
    const/4 v0, 0x0
    return v0
    .catch Ljava/lang/IllegalArgumentException; {:start .. :end} :handler
    .catch Ljava/lang/IllegalStateException; {:start .. :end} :handler
.end method

.method public static catchAllOnly()I
    .locals 1
    :start
    invoke-static {}, Lexceptions/Support;->fail()V
    :end
    goto :done
    :handler
    move-exception v0
    const/4 v0, 0x1
    return v0
    :done
    const/4 v0, 0x0
    return v0
    .catchall {:start .. :end} :handler
.end method
