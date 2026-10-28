.class public Lexceptions/AdjacentTry;
.super Ljava/lang/Object;

.method public static two()V
    .locals 1
    :first_start
    invoke-static {}, Lexceptions/Support;->first()V
    :first_end
    goto :second_start
    :second_start
    invoke-static {}, Lexceptions/Support;->second()V
    :second_end
    goto :done
    :first_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtFirst()V
    return-void
    :second_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtSecond()V
    return-void
    :done
    invoke-static {}, Lexceptions/Support;->outside()V
    return-void
    .catch Ljava/lang/IllegalArgumentException; {:first_start .. :first_end} :first_handler
    .catch Ljava/lang/IllegalStateException; {:second_start .. :second_end} :second_handler
.end method

.method public static three()V
    .locals 1
    :first_start
    invoke-static {}, Lexceptions/Support;->first()V
    :first_end
    goto :second_start
    :second_start
    invoke-static {}, Lexceptions/Support;->second()V
    :second_end
    goto :third_start
    :third_start
    invoke-static {}, Lexceptions/Support;->third()V
    :third_end
    goto :done
    :first_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtFirst()V
    return-void
    :second_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtSecond()V
    return-void
    :third_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtThird()V
    return-void
    :done
    invoke-static {}, Lexceptions/Support;->outside()V
    return-void
    .catch Ljava/lang/IllegalArgumentException; {:first_start .. :first_end} :first_handler
    .catch Ljava/lang/IllegalStateException; {:second_start .. :second_end} :second_handler
    .catch Ljava/lang/UnsupportedOperationException; {:third_start .. :third_end} :third_handler
.end method

.method public static sameType()V
    .locals 1
    :first_start
    invoke-static {}, Lexceptions/Support;->first()V
    :first_end
    goto :second_start
    :second_start
    invoke-static {}, Lexceptions/Support;->second()V
    :second_end
    goto :done
    :first_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtFirst()V
    return-void
    :second_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtSecond()V
    return-void
    :done
    invoke-static {}, Lexceptions/Support;->outside()V
    return-void
    .catch Ljava/lang/IllegalArgumentException; {:first_start .. :first_end} :first_handler
    .catch Ljava/lang/IllegalArgumentException; {:second_start .. :second_end} :second_handler
.end method

.method public static nested()V
    .locals 1
    :outer_start
    invoke-static {}, Lexceptions/Support;->first()V
    :inner_start
    invoke-static {}, Lexceptions/Support;->second()V
    :inner_end
    invoke-static {}, Lexceptions/Support;->outerTail()V
    :outer_end
    return-void
    :inner_handler
    move-exception v0
    return-void
    :outer_handler
    move-exception v0
    invoke-static {}, Lexceptions/Support;->caughtFirst()V
    return-void
    .catch Ljava/lang/IllegalArgumentException; {:outer_start .. :inner_start} :outer_handler
    .catch Ljava/lang/IllegalStateException; {:inner_start .. :inner_end} :inner_handler
    .catch Ljava/lang/IllegalArgumentException; {:inner_start .. :inner_end} :outer_handler
    .catch Ljava/lang/IllegalArgumentException; {:inner_end .. :outer_end} :outer_handler
.end method
