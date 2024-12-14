/* DO NOT EDIT THIS FILE - it is machine generated */
#include <jni.h>
/* Header for class HelloWorld */

#ifndef _Included_HelloWorld
#define _Included_HelloWorld
#ifdef __cplusplus
extern "C" {
#endif
/*
 * Class:     HelloWorld
 * Method:    hello
 * Signature: (Ljava/lang/String;)Ljava/lang/String;
 */
JNIEXPORT jstring JNICALL Java_HelloWorld_hello
  (JNIEnv *, jclass, jstring);

/*
 * Class:     HelloWorld
 * Method:    helloByte
 * Signature: ([B)[B
 */
JNIEXPORT jbyteArray JNICALL Java_HelloWorld_helloByte
  (JNIEnv *, jclass, jbyteArray);

/*
 * Class:     HelloWorld
 * Method:    factAndCallMeBack
 * Signature: (ILHelloWorld;)V
 */
JNIEXPORT void JNICALL Java_HelloWorld_factAndCallMeBack
  (JNIEnv *, jclass, jint, jobject);

/*
 * Class:     HelloWorld
 * Method:    counterNew
 * Signature: (LHelloWorld;)J
 */
JNIEXPORT jlong JNICALL Java_HelloWorld_counterNew
  (JNIEnv *, jclass, jobject);

/*
 * Class:     HelloWorld
 * Method:    counterIncrement
 * Signature: (J)V
 */
JNIEXPORT void JNICALL Java_HelloWorld_counterIncrement
  (JNIEnv *, jclass, jlong);

/*
 * Class:     HelloWorld
 * Method:    counterDestroy
 * Signature: (J)V
 */
JNIEXPORT void JNICALL Java_HelloWorld_counterDestroy
  (JNIEnv *, jclass, jlong);

/*
 * Class:     HelloWorld
 * Method:    asyncComputation
 * Signature: (LHelloWorld;)V
 */
JNIEXPORT void JNICALL Java_HelloWorld_asyncComputation
  (JNIEnv *, jclass, jobject);

#ifdef __cplusplus
}
#endif
#endif
